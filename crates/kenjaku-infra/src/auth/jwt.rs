//! JWT validator (Phase 3c.1).
//!
//! Wraps [`jsonwebtoken`] v10 to validate tenant-scoped bearer tokens.
//!
//! # Security invariants
//!
//! 1. **Asymmetric algorithm allowlist (defense in depth).**
//!    - Layer 1: [`kenjaku_core::config::JwtAlgorithm`] is an enum that
//!      literally cannot represent `HS*`, `PS*`, or `none`. The YAML
//!      parser rejects those values.
//!    - Layer 2: [`Validation::algorithms`] is pinned to a single-element
//!      vec derived from that enum. The `jsonwebtoken` decoder consults
//!      this list and rejects any JWT whose `alg` header is not in it.
//!
//!    This is the canonical mitigation for the classic "sign HS256 with
//!    the public key as secret" algorithm-confusion attack.
//!
//! 2. **Generic error surface.** Every decoder failure — expired,
//!    wrong issuer, wrong audience, tampered signature, malformed
//!    input, wrong algorithm, missing claim — flattens to
//!    [`Error::TenantAuth`] with
//!    [`AuthErrorCode::Unauthorized`]. The specific reason is logged
//!    at `tracing::debug!` for operators but never returned to the
//!    caller, preventing enumeration attacks where an attacker learns
//!    which check a token is closest to passing.
//!
//! 3. **Constant-time signature compare.** Provided by
//!    [`jsonwebtoken::decode`]. We never hand-roll signature
//!    verification — bypassing this invariant is exactly how most
//!    classic JWT libs got their CVEs.
//!
//! 4. **Public key sourcing is a bootstrap/DI policy, not enforced by
//!    this module.** The application should read the PEM bytes from a
//!    configured filesystem path once at startup and pass those bytes
//!    into [`JwtValidator`]. This module accepts caller-supplied PEM
//!    bytes and does not itself enforce "from filesystem only" or
//!    "never from env". Keeping file-based loading in the composition
//!    layer helps avoid secrets-in-code leaks and accidental exposure.
//!
//! 5. **`iss`, `aud`, `exp`, `iat` required.** Tokens missing any of
//!    these are rejected. `nbf` honored when present.
//!
//! 6. **`plan_tier` in claims is advisory only.** Slice 3c.2's auth
//!    middleware sources the effective `plan_tier` from the tenants
//!    cache (DB), not from the claim. Keeping the claim field here
//!    for forward-compat and diagnostics, but DO NOT use it for
//!    authorization decisions.

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use tracing::debug;

use kenjaku_core::config::{JwtAlgorithm, JwtConfig};
use kenjaku_core::error::{AuthErrorCode, Error, Result};
use kenjaku_core::types::tenant::{PlanTier, PrincipalId, TenantId};

/// Validated tenant claims extracted from a JWT.
///
/// Constructed only by [`JwtValidator::validate`] after signature +
/// iss/aud/exp/iat/nbf checks pass. `plan_tier` here is a **diagnostic
/// hint**; the auth middleware (slice 3c.2) must source the effective
/// plan tier from the tenants DB row, not from this claim. See the
/// module-level security invariants.
///
/// `tenant_id` and `principal_id` are the typed newtypes from
/// `kenjaku_core::types::tenant`. Their serde `try_from = "String"`
/// impls run the Phase 3a charset + length validation during JWT
/// deserialization — so a token carrying, e.g., `"tenant_id":
/// "../etc/passwd"` is rejected by the decoder with the same generic
/// [`AuthErrorCode::Unauthorized`] as any other malformed token. No
/// caller is trusted to remember to re-validate.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TenantClaims {
    /// Tenant identifier. Validated by [`TenantId`]'s serde impl
    /// during JWT decode — invalid charset or length rejects as a
    /// generic `Unauthorized` before the claim ever reaches a
    /// handler.
    pub tenant_id: TenantId,
    /// Optional principal identifier (user / service account).
    /// Validated by [`PrincipalId`]'s serde impl — same rules as
    /// `tenant_id`.
    ///
    /// `skip_serializing_if` keeps the claim absent on the wire when
    /// `None` — the JWT spec rejects null-valued optional claims, and
    /// `JwtMinter::mint` would otherwise emit `"principal_id": null`
    /// which round-trips fine but is non-canonical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<PrincipalId>,
    /// Optional plan-tier hint — DO NOT use for authorization
    /// decisions. See module docs. Tokens may omit this advisory
    /// claim without failing validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_tier: Option<PlanTier>,
    /// Expiration time (Unix seconds). Required.
    pub exp: u64,
    /// Issuer. Required; validated against [`JwtConfig::issuer`].
    pub iss: String,
    /// Audience. Required; validated against [`JwtConfig::audience`].
    pub aud: String,
    /// Issued-at time (Unix seconds). Required per
    /// [`Validation::set_required_spec_claims`].
    pub iat: u64,
    /// Not-before time (Unix seconds). Optional per JWT spec; when
    /// present `jsonwebtoken` rejects tokens whose `nbf > now + leeway`.
    ///
    /// `skip_serializing_if` — see `principal_id` above. Minting a
    /// token with `nbf: null` makes `jsonwebtoken::decode` reject the
    /// token because the spec does not permit nullable numeric claims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
}

/// Stateful JWT validator. Construct once at startup, share via
/// `Arc<JwtValidator>`.
///
/// # `Debug` redaction
///
/// The custom `Debug` impl deliberately hides the `decoding_key` field
/// (which wraps public-key material) and the `validation` internals
/// (which list the allowed algorithms, required claims, etc.). Logging
/// a [`JwtValidator`] prints only `JwtValidator { algorithm: RS256 }` —
/// enough for a "which validator ran?" trace, nothing an attacker could
/// weaponize if a stray `?` debug format lands in a log line.
pub struct JwtValidator {
    decoding_key: DecodingKey,
    validation: Validation,
    algorithm: JwtAlgorithm,
}

impl std::fmt::Debug for JwtValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtValidator")
            .field("algorithm", &self.algorithm)
            .finish_non_exhaustive()
    }
}

impl JwtValidator {
    /// Build a validator from config + pre-loaded PEM bytes.
    ///
    /// **Filesystem access is deliberately NOT performed here.** The
    /// validator is a pure parser/verifier; reading
    /// [`JwtConfig::public_key_path`] from disk is the DI layer's job
    /// (see `kenjaku-server/src/main.rs` in slice 3c.2). This
    /// separation:
    ///
    /// - Keeps the security-critical module filesystem-free, which
    ///   narrows the CWE-22 attack surface to the server bootstrap
    ///   where secrets and the DB URL are also resolved — one
    ///   location to audit, not many.
    /// - Makes the validator trivially testable with in-memory PEM
    ///   bytes (no tempfile dance), matching the test fixtures below.
    /// - Lets the server layer apply deployment-specific file-read
    ///   policy (size caps, ownership checks, kubelet secret mounts)
    ///   once for every security file, not per-subsystem.
    ///
    /// Configures [`jsonwebtoken::Validation`] with:
    /// - `algorithms = vec![config.algorithm.to_jwt_alg()]` (single-
    ///   element allowlist — cannot be downgraded at request time)
    /// - required spec claims: `exp`, `iss`, `aud`, `iat`
    /// - `set_issuer(&[config.issuer])`, `set_audience(&[config.audience])`
    /// - `leeway = config.clock_skew_secs`
    /// - `validate_exp = true`, `validate_nbf = true`
    ///
    /// # Errors
    ///
    /// Returns `Error::Config` on PEM parse failure (e.g. bytes aren't
    /// a public key, wrong key type for the chosen algorithm).
    ///
    /// Note: we deliberately surface `Config` (not `TenantAuth`) here
    /// because this runs at startup — an operator needs the specific
    /// reason to fix their deployment. Token-validation failures at
    /// request time do flatten to `TenantAuth`.
    pub fn new(config: &JwtConfig, public_key_pem: &[u8]) -> Result<Self> {
        if public_key_pem.is_empty() {
            return Err(Error::Config("JWT public key PEM is empty".to_string()));
        }

        let decoding_key = build_decoding_key(public_key_pem, config.algorithm).map_err(|e| {
            Error::Config(format!(
                "failed to parse JWT public key as {:?}: {e}",
                config.algorithm
            ))
        })?;

        let alg = algorithm_to_jwt(config.algorithm);
        let mut validation = Validation::new(alg);
        // Pin allowlist to the single configured algorithm. This is
        // the second half of the defense-in-depth against algorithm
        // confusion — even if a malicious token shows up with
        // `alg: HS256`, the decoder will reject before touching the key.
        validation.algorithms = vec![alg];
        validation.set_required_spec_claims(&["exp", "iss", "aud", "iat"]);
        validation.set_issuer(&[&config.issuer]);
        validation.set_audience(&[&config.audience]);
        validation.leeway = config.clock_skew_secs;
        validation.validate_exp = true;
        validation.validate_nbf = true;

        Ok(Self {
            decoding_key,
            validation,
            algorithm: config.algorithm,
        })
    }

    /// Validate `token` and return the tenant claims.
    ///
    /// Every failure — signature mismatch, expired, wrong iss/aud,
    /// malformed, wrong algorithm, missing required claim — flattens
    /// to a single [`Error::TenantAuth(AuthErrorCode::Unauthorized)`]
    /// value. The specific library error is logged at
    /// `tracing::debug!` for operators; never returned.
    pub fn validate(&self, token: &str) -> Result<TenantClaims> {
        match decode::<TenantClaims>(token, &self.decoding_key, &self.validation) {
            Ok(data) => Ok(data.claims),
            Err(e) => {
                // Log the precise reason for operators. `e` may include
                // "InvalidSignature", "ExpiredSignature", "InvalidIssuer",
                // etc. — useful in dev/diagnostic logs, not safe in the
                // user-facing error path.
                // `tracing`'s `?` formatter calls `Debug` directly on
                // `e.kind()`, avoiding a per-failure `String` allocation
                // on what is an attacker-triggerable path.
                debug!(
                    target: "kenjaku_infra::auth",
                    kind = ?e.kind(),
                    algorithm = self.algorithm.as_str(),
                    "JWT validation failed"
                );
                Err(Error::TenantAuth(AuthErrorCode::Unauthorized))
            }
        }
    }
}

// ---------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------

/// Map the typed [`JwtAlgorithm`] to the library's `Algorithm` enum.
///
/// This function is total — every `JwtAlgorithm` variant has a
/// one-to-one mapping. It's deliberately NOT a `From` impl because
/// importing crates should treat `JwtAlgorithm` as the source of truth
/// and not accidentally pick up `jsonwebtoken::Algorithm` values that
/// bypass the allowlist.
fn algorithm_to_jwt(alg: JwtAlgorithm) -> Algorithm {
    match alg {
        JwtAlgorithm::RS256 => Algorithm::RS256,
        JwtAlgorithm::RS384 => Algorithm::RS384,
        JwtAlgorithm::RS512 => Algorithm::RS512,
        JwtAlgorithm::ES256 => Algorithm::ES256,
        JwtAlgorithm::ES384 => Algorithm::ES384,
    }
}

/// Parse a public-key PEM into a `DecodingKey` for the given algorithm.
fn build_decoding_key(
    pem_bytes: &[u8],
    alg: JwtAlgorithm,
) -> std::result::Result<DecodingKey, jsonwebtoken::errors::Error> {
    match alg {
        JwtAlgorithm::RS256 | JwtAlgorithm::RS384 | JwtAlgorithm::RS512 => {
            DecodingKey::from_rsa_pem(pem_bytes)
        }
        JwtAlgorithm::ES256 | JwtAlgorithm::ES384 => DecodingKey::from_ec_pem(pem_bytes),
    }
}

// =====================================================================
// JwtMinter — server-side JWT issuance for POST /api/v1/auth/login.
// =====================================================================

/// Mints RS256/ES* JWTs symmetric with [`JwtValidator`]'s expectations.
///
/// Construction takes the PRIVATE key PEM bytes + a [`JwtConfig`] (for
/// `issuer`, `audience`, `algorithm`, `ttl_seconds`). The claims shape
/// matches [`TenantClaims`] exactly — so a freshly minted token can be
/// fed straight back through `JwtValidator::validate` in the same
/// process for round-trip tests, and in production every minted token
/// passes the middleware check at the next request.
///
/// # `Debug` redaction
///
/// Same policy as `JwtValidator` — the custom `Debug` impl hides the
/// `encoding_key` field. Logging a minter prints only
/// `JwtMinter { algorithm: RS256, ttl_seconds: 86400 }`.
pub struct JwtMinter {
    encoding_key: EncodingKey,
    algorithm: JwtAlgorithm,
    issuer: String,
    audience: String,
    ttl_seconds: u64,
}

impl std::fmt::Debug for JwtMinter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtMinter")
            .field("algorithm", &self.algorithm)
            .field("ttl_seconds", &self.ttl_seconds)
            .finish_non_exhaustive()
    }
}

impl JwtMinter {
    /// Build a minter from config + pre-loaded private-key PEM bytes.
    ///
    /// Like [`JwtValidator::new`], this module is deliberately
    /// filesystem-free. The server bootstrap reads the private key
    /// from `config.tenancy.jwt.private_key_path` and hands bytes in.
    pub fn new(config: &JwtConfig, private_key_pem: &[u8]) -> Result<Self> {
        if private_key_pem.is_empty() {
            return Err(Error::Config("JWT private key PEM is empty".to_string()));
        }
        let encoding_key = build_encoding_key(private_key_pem, config.algorithm).map_err(|e| {
            Error::Config(format!(
                "failed to parse JWT private key as {:?}: {e}",
                config.algorithm
            ))
        })?;
        Ok(Self {
            encoding_key,
            algorithm: config.algorithm,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            ttl_seconds: config.ttl_seconds,
        })
    }

    /// The algorithm this minter produces tokens under.
    pub fn algorithm(&self) -> JwtAlgorithm {
        self.algorithm
    }

    /// Mint a token for `(tenant_id, principal_id, plan_tier)` with a
    /// `ttl` override in seconds; `None` uses `ttl_seconds` from config.
    ///
    /// Returns `(token, exp_utc)` so handlers can both echo the
    /// expiration to the client and set it in their own bookkeeping
    /// without re-parsing the JWT.
    pub fn mint(
        &self,
        tenant_id: &TenantId,
        principal_id: Option<&PrincipalId>,
        plan_tier: Option<PlanTier>,
        ttl: Option<u64>,
    ) -> Result<(String, DateTime<Utc>)> {
        let now = Utc::now();
        let ttl = ttl.unwrap_or(self.ttl_seconds);
        let exp = now + chrono::Duration::seconds(ttl as i64);

        let claims = TenantClaims {
            tenant_id: tenant_id.clone(),
            principal_id: principal_id.cloned(),
            plan_tier,
            exp: exp.timestamp() as u64,
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now.timestamp() as u64,
            nbf: None,
        };

        let header = Header::new(algorithm_to_jwt(self.algorithm));
        let token = encode(&header, &claims, &self.encoding_key).map_err(|e| {
            // Startup-error severity: encoding should only fail on a
            // malformed key — which the DI layer caught earlier.
            // Surface as Internal so the handler returns a generic 500
            // rather than leaking crypto internals.
            debug!(target: "kenjaku_infra::auth", kind = ?e.kind(), "JWT mint failed");
            Error::Internal("JWT mint failed".to_string())
        })?;
        Ok((token, exp))
    }
}

/// Parse a PRIVATE-key PEM into an `EncodingKey` for the given
/// algorithm. Mirrors [`build_decoding_key`].
fn build_encoding_key(
    pem_bytes: &[u8],
    alg: JwtAlgorithm,
) -> std::result::Result<EncodingKey, jsonwebtoken::errors::Error> {
    match alg {
        JwtAlgorithm::RS256 | JwtAlgorithm::RS384 | JwtAlgorithm::RS512 => {
            EncodingKey::from_rsa_pem(pem_bytes)
        }
        JwtAlgorithm::ES256 | JwtAlgorithm::ES384 => EncodingKey::from_ec_pem(pem_bytes),
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::OnceLock;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

    // ---------- Fixtures --------------------------------------------------
    //
    // Historically this module embedded a hardcoded RSA-2048 PEM pair.
    // PR #17 Copilot review #5 flagged that as a CWE-798 hardcoded-
    // credentials smell. Migrated to runtime keygen matching the
    // pattern already in `kenjaku-api` (`tests/auth_flow.rs` and
    // `src/middleware/auth.rs` tests). ~200ms keygen cost amortized
    // across all tests via `OnceLock`; seeded RNG keeps the
    // materialized key deterministic + reproducible.

    /// Process-wide test keypair.
    struct TestKeypair {
        private_pem: String,
        public_pem: String,
    }

    fn keypair() -> &'static TestKeypair {
        static KP: OnceLock<TestKeypair> = OnceLock::new();
        KP.get_or_init(|| {
            let mut rng = StdRng::seed_from_u64(0xDEADBEEF_CAFEBABE);
            let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
            let pub_key = priv_key.to_public_key();
            let private_pem = priv_key
                .to_pkcs8_pem(LineEnding::LF)
                .expect("pkcs8 priv pem")
                .to_string();
            let public_pem = pub_key
                .to_public_key_pem(LineEnding::LF)
                .expect("spki pub pem");
            TestKeypair {
                private_pem,
                public_pem,
            }
        })
    }

    const TEST_ISSUER: &str = "kenjaku-test-issuer";
    const TEST_AUDIENCE: &str = "kenjaku-test-audience";

    /// Build the validator for the test keypair + test issuer/audience.
    ///
    /// Construction takes PEM bytes directly — the production server
    /// layer is responsible for reading those bytes from
    /// `config.tenancy.jwt.public_key_path`. See the
    /// [`JwtValidator::new`] rustdoc for the rationale.
    fn make_validator() -> JwtValidator {
        let cfg = JwtConfig {
            // `public_key_path` is now informational only for the
            // validator — it reads bytes it was handed. Still populate
            // it to document the wire shape.
            issuer: TEST_ISSUER.to_string(),
            audience: TEST_AUDIENCE.to_string(),
            public_key_path: "<test-only>".to_string(),
            private_key_path: "<test-only>".to_string(),
            ttl_seconds: 3600,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        };
        JwtValidator::new(&cfg, keypair().public_pem.as_bytes()).expect("validator constructed")
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Serialize a claims JSON map and sign it with the test RSA key
    /// as RS256. Callers pass a `serde_json::Value` so they can omit
    /// or tamper with specific fields.
    fn mint_rs256(claims: &serde_json::Value) -> String {
        let enc_key = EncodingKey::from_rsa_pem(keypair().private_pem.as_bytes())
            .expect("encoding key from private PEM");
        encode(&Header::new(Algorithm::RS256), claims, &enc_key).expect("mint rs256")
    }

    /// Build a full claims object with defaults (all required fields
    /// present, iat=now, exp=now+300). Merge caller-supplied overrides.
    fn default_claims() -> serde_json::Value {
        let now = now_secs();
        serde_json::json!({
            "tenant_id": "acme",
            "principal_id": "user-42",
            "plan_tier": "pro",
            "exp": now + 300,
            "iat": now,
            "iss": TEST_ISSUER,
            "aud": TEST_AUDIENCE,
        })
    }

    // ---------- Tests -----------------------------------------------------

    // Test 1/12 — valid RS256 token accepts.
    #[test]
    fn test_01_valid_rs256_token_accepts() {
        let v = make_validator();
        let token = mint_rs256(&default_claims());
        let claims = v.validate(&token).expect("valid token must pass");
        assert_eq!(claims.tenant_id.as_str(), "acme");
        assert_eq!(
            claims.principal_id.as_ref().map(|p| p.as_str()),
            Some("user-42")
        );
        assert_eq!(claims.plan_tier, Some(PlanTier::Pro));
        assert_eq!(claims.iss, TEST_ISSUER);
        assert_eq!(claims.aud, TEST_AUDIENCE);
    }

    // Test 2/12 — expired `exp` rejects.
    #[test]
    fn test_02_expired_token_rejects() {
        let v = make_validator();
        let now = now_secs();
        let mut c = default_claims();
        c["exp"] = serde_json::json!(now - 1_000);
        c["iat"] = serde_json::json!(now - 2_000);
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("expired must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 3/12 — `nbf` in future rejects.
    #[test]
    fn test_03_nbf_in_future_rejects() {
        let v = make_validator();
        let now = now_secs();
        let mut c = default_claims();
        // nbf=now+1h — well beyond the 5s leeway configured by make_validator.
        c["nbf"] = serde_json::json!(now + 3_600);
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("nbf-future must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 4/12 — wrong `iss` rejects.
    #[test]
    fn test_04_wrong_issuer_rejects() {
        let v = make_validator();
        let mut c = default_claims();
        c["iss"] = serde_json::json!("hostile-issuer");
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("wrong iss must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 5/12 — wrong `aud` rejects.
    #[test]
    fn test_05_wrong_audience_rejects() {
        let v = make_validator();
        let mut c = default_claims();
        c["aud"] = serde_json::json!("other-api");
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("wrong aud must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 6/12 — tampered signature rejects.
    #[test]
    fn test_06_tampered_signature_rejects() {
        let v = make_validator();
        let token = mint_rs256(&default_claims());
        // Flip the last byte of the signature segment. The compact JWS
        // layout is `<header>.<payload>.<signature>`.
        let mut parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let mut sig = URL_SAFE_NO_PAD.decode(parts[2]).expect("decode sig");
        let last = sig.len() - 1;
        sig[last] ^= 0x01;
        let tampered = URL_SAFE_NO_PAD.encode(&sig);
        parts[2] = &tampered;
        let bad = parts.join(".");
        let err = v.validate(&bad).expect_err("tampered sig must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 7/12 — `alg: none` rejects. **SECURITY-CRITICAL.**
    //
    // Hand-crafts an unsigned token with the `none` algorithm and
    // asserts it's rejected. The jsonwebtoken decoder must refuse
    // because the `Validation::algorithms` allowlist does not contain
    // `Algorithm::None` (which doesn't even exist as a variant in
    // jsonwebtoken v10). We build the token by hand — `encode()` will
    // not emit `alg:none` for us.
    #[test]
    fn test_07_alg_none_rejects() {
        let v = make_validator();
        let header_b64 = URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = serde_json::to_string(&default_claims()).unwrap();
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        // Per RFC7515 §6.1 an unsigned token's signature segment is the
        // empty string. Keep the trailing dot so the JWS compact shape
        // is preserved.
        let token = format!("{header_b64}.{payload_b64}.");
        let err = v
            .validate(&token)
            .expect_err("SECURITY: alg:none tokens MUST be rejected by the algorithm allowlist");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 8/12 — algorithm confusion: HS256 signed with the public
    // key bytes as the HMAC secret. **SECURITY-CRITICAL.**
    //
    // This is the textbook JWT exploit: if a server accepts both
    // asymmetric and symmetric algorithms and uses the same key
    // material for both, an attacker can take the public key, HMAC-
    // sign their own claims with it, and present the result. Our
    // `Validation::algorithms` pin prevents this — HS256 isn't in
    // the single-element vec, so the decoder rejects before any
    // key material is consulted.
    #[test]
    fn test_08_alg_hs256_confusion_rejects() {
        let v = make_validator();
        // HMAC-sign with the public PEM as the shared secret — the
        // classic algorithm-confusion attack.
        let hs_key = EncodingKey::from_secret(keypair().public_pem.as_bytes());
        let token = encode(&Header::new(Algorithm::HS256), &default_claims(), &hs_key)
            .expect("hs256 mint succeeds — but validator must still reject");
        let err = v.validate(&token).expect_err(
            "SECURITY: HS256 with public key as secret MUST be rejected by the algorithm allowlist",
        );
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 9/12 — missing `tenant_id` claim rejects.
    #[test]
    fn test_09_missing_tenant_id_claim_rejects() {
        let v = make_validator();
        let mut c = default_claims();
        c.as_object_mut().unwrap().remove("tenant_id");
        let token = mint_rs256(&c);
        // The decoder's serde step fails because TenantClaims.tenant_id
        // is non-Option. Result: generic Unauthorized (no claim name
        // leaked).
        let err = v.validate(&token).expect_err("missing tenant_id rejects");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 10/12 — missing `exp` rejects.
    #[test]
    fn test_10_missing_exp_rejects() {
        let v = make_validator();
        let mut c = default_claims();
        c.as_object_mut().unwrap().remove("exp");
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("missing exp rejects");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 11/12 — malformed JWT rejects.
    #[test]
    fn test_11_malformed_jwt_rejects() {
        let v = make_validator();
        for bad in [
            "not-a-jwt-at-all",
            "only.two",
            "",
            "....",
            "!!!.???.***",
            "aGVsbG8.d29ybGQ", // two segments, no signature
        ] {
            let err = v.validate(bad).expect_err("malformed must reject");
            assert!(
                matches!(err, Error::TenantAuth(AuthErrorCode::Unauthorized)),
                "expected Unauthorized for malformed input {bad:?}"
            );
        }
    }

    // Test 12/12 — wrong-length signature rejects.
    //
    // RSA-2048 signatures are 256 bytes. Truncating or padding the
    // signature segment must not lead to an "OK" result via a
    // short-circuit in the decoder. (In practice `jsonwebtoken`'s
    // ring backend treats this as a signature failure.)
    #[test]
    fn test_12_wrong_length_signature_rejects() {
        let v = make_validator();
        let token = mint_rs256(&default_claims());
        let mut parts: Vec<&str> = token.split('.').collect();
        // Truncate signature to 32 bytes (half of a usable ECDSA sig,
        // way too short for RSA-2048).
        let sig = URL_SAFE_NO_PAD.decode(parts[2]).expect("decode");
        let truncated = URL_SAFE_NO_PAD.encode(&sig[..32]);
        parts[2] = &truncated;
        let bad = parts.join(".");
        let err = v.validate(&bad).expect_err("short sig rejects");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));

        // Also pad with garbage (256 + 16 bytes) — must still reject.
        let mut padded = URL_SAFE_NO_PAD
            .decode(token.rsplit_once('.').unwrap().1)
            .expect("decode orig sig");
        padded.extend_from_slice(&[0xAAu8; 16]);
        let padded_b64 = URL_SAFE_NO_PAD.encode(&padded);
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[2] = &padded_b64;
        let bad2 = parts.join(".");
        let err2 = v.validate(&bad2).expect_err("padded sig rejects");
        assert!(matches!(
            err2,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // ---------- Post-Copilot-review hardening (3c.1 fix turn) -------------

    // Test 13 — newtype claim validation is enforced at JWT decode.
    //
    // Follow-up to PR #16 Copilot feedback: `TenantClaims.tenant_id`
    // was hardened from `String` to the validated `TenantId` newtype.
    // Its serde `try_from = "String"` impl runs the Phase 3a charset
    // + length allowlist as part of decode, so a token carrying an
    // invalid identifier is rejected as a generic `Unauthorized` —
    // no handler can forget to re-validate, because the claim
    // literally cannot be deserialized as a malformed string.
    //
    // We cover three slices of the validation surface:
    // (a) invalid charset — must reject
    // (b) empty string — must reject
    // (c) boundary length (128 ok / 129 rejects)
    #[test]
    fn test_13_newtype_tenant_id_validation_at_decode_time() {
        let v = make_validator();

        // (a) invalid charset — path traversal seed char.
        let mut c = default_claims();
        c["tenant_id"] = serde_json::json!("../etc/passwd");
        let token = mint_rs256(&c);
        let err = v
            .validate(&token)
            .expect_err("invalid-charset tenant_id must reject at decode");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));

        // (a') shell-meta char.
        c["tenant_id"] = serde_json::json!("acme;rm -rf /");
        let token = mint_rs256(&c);
        let err = v
            .validate(&token)
            .expect_err("shell-meta tenant_id must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));

        // (b) empty string.
        c["tenant_id"] = serde_json::json!("");
        let token = mint_rs256(&c);
        let err = v.validate(&token).expect_err("empty tenant_id must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));

        // (c) exactly MAX_ID_LEN (128) accepts.
        let max_ok = "a".repeat(kenjaku_core::types::tenant::MAX_ID_LEN);
        c["tenant_id"] = serde_json::json!(max_ok);
        let token = mint_rs256(&c);
        let claims = v
            .validate(&token)
            .expect("tenant_id at exactly MAX_ID_LEN must accept");
        assert_eq!(claims.tenant_id.as_str(), max_ok);

        // (c') MAX_ID_LEN + 1 rejects.
        let too_long = "a".repeat(kenjaku_core::types::tenant::MAX_ID_LEN + 1);
        c["tenant_id"] = serde_json::json!(too_long);
        let token = mint_rs256(&c);
        let err = v
            .validate(&token)
            .expect_err("over-MAX_ID_LEN tenant_id must reject");
        assert!(matches!(
            err,
            Error::TenantAuth(AuthErrorCode::Unauthorized)
        ));
    }

    // Test 14 — plan_tier is advisory; tokens may omit it and still
    // validate.
    //
    // Follow-up to PR #16 Copilot feedback: `plan_tier` was hardened
    // from a required claim into `#[serde(default)] Option<PlanTier>`.
    // Issuers that don't populate it are fine; the middleware (3c.2)
    // must source the effective plan-tier from the tenants DB row
    // anyway, so the claim is strictly a diagnostic hint.
    #[test]
    fn test_14_token_without_plan_tier_accepts() {
        let v = make_validator();
        let mut c = default_claims();
        c.as_object_mut().unwrap().remove("plan_tier");
        let token = mint_rs256(&c);
        let claims = v
            .validate(&token)
            .expect("token without plan_tier must validate");
        assert!(
            claims.plan_tier.is_none(),
            "missing plan_tier claim must deserialize as None, not a default tier"
        );
        // Other fields unaffected.
        assert_eq!(claims.tenant_id.as_str(), "acme");
    }

    // ---------- Additional defensive tests --------------------------------

    // Defense-in-depth: construction fails closed if the PEM bytes are
    // empty. This is the DI-layer contract — the server binary is
    // responsible for reading a non-empty PEM from disk, but if it
    // ever passes `&[]` by mistake, we fail fast with a specific
    // error rather than carrying on with a broken validator.
    #[test]
    fn test_new_fails_on_empty_pem_bytes() {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<unused>".into(),
            private_key_path: "<unused>".into(),
            ttl_seconds: 3600,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 30,
        };
        let err = JwtValidator::new(&cfg, b"").expect_err("empty pem must fail");
        // Startup errors are Config, not TenantAuth — operator needs
        // the specific reason here.
        assert!(matches!(err, Error::Config(_)));
    }

    // Defense-in-depth: construction fails closed if the PEM is garbage.
    #[test]
    fn test_new_fails_on_garbage_pem() {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<unused>".into(),
            private_key_path: "<unused>".into(),
            ttl_seconds: 3600,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 30,
        };
        let err = JwtValidator::new(&cfg, b"not a pem, not even close")
            .expect_err("garbage pem must fail");
        assert!(matches!(err, Error::Config(_)));
    }

    // Defense-in-depth: algorithm/key-type mismatch. A caller that
    // supplies an RSA PEM but declares ES256 must be rejected — the
    // library's key-to-algorithm cross-check catches this, but we
    // confirm it rather than assume.
    #[test]
    fn test_new_fails_when_algorithm_and_key_type_mismatch() {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<unused>".into(),
            private_key_path: "<unused>".into(),
            ttl_seconds: 3600,
            algorithm: JwtAlgorithm::ES256, // declares EC...
            clock_skew_secs: 30,
        };
        // ...but bytes are RSA.
        let err = JwtValidator::new(&cfg, keypair().public_pem.as_bytes())
            .expect_err("RSA PEM for ES256 must fail");
        assert!(matches!(err, Error::Config(_)));
    }

    // Defense-in-depth: any skew of `clock_skew_secs` tolerance still
    // rejects tokens expired by >> leeway.
    #[test]
    fn test_clock_skew_tolerates_small_drift_but_rejects_large_drift() {
        // 60s leeway — tolerate recent expiries but reject an hour ago.
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<unused>".into(),
            private_key_path: "<unused>".into(),
            ttl_seconds: 3600,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 60,
        };
        let v = JwtValidator::new(&cfg, keypair().public_pem.as_bytes()).unwrap();

        // Just-expired within leeway: must accept.
        let now = now_secs();
        let mut c = default_claims();
        c["exp"] = serde_json::json!(now - 5); // expired 5s ago
        c["iat"] = serde_json::json!(now - 10);
        let token = mint_rs256(&c);
        assert!(v.validate(&token).is_ok(), "within-leeway must pass");

        // Expired by far more than leeway: must reject.
        c["exp"] = serde_json::json!(now - Duration::from_secs(3600).as_secs());
        c["iat"] = serde_json::json!(now - Duration::from_secs(7200).as_secs());
        let old = mint_rs256(&c);
        assert!(v.validate(&old).is_err(), "beyond-leeway must reject");
    }

    // =================================================================
    // JwtMinter tests — auth-login-rbac
    // =================================================================

    fn make_minter(ttl: u64) -> JwtMinter {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<test>".into(),
            private_key_path: "<test>".into(),
            ttl_seconds: ttl,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        };
        JwtMinter::new(&cfg, keypair().private_pem.as_bytes()).expect("minter constructed")
    }

    #[test]
    fn minter_roundtrips_token_through_validator() {
        let minter = make_minter(300);
        let v = make_validator();
        let tid = TenantId::new("acme").unwrap();
        let pid = PrincipalId::new("user-42").unwrap();
        let (token, exp) = minter
            .mint(&tid, Some(&pid), Some(PlanTier::Pro), None)
            .expect("mint");

        let claims = v.validate(&token).expect("validate minted token");
        assert_eq!(claims.tenant_id.as_str(), "acme");
        assert_eq!(
            claims.principal_id.as_ref().map(|p| p.as_str()),
            Some("user-42")
        );
        assert_eq!(claims.plan_tier, Some(PlanTier::Pro));
        assert_eq!(claims.iss, TEST_ISSUER);
        assert_eq!(claims.aud, TEST_AUDIENCE);

        // Returned exp matches the claim (±1s rounding).
        let exp_claim = claims.exp as i64;
        let exp_returned = exp.timestamp();
        assert!(
            (exp_returned - exp_claim).abs() <= 1,
            "returned exp ({exp_returned}) must match claim ({exp_claim})"
        );
    }

    #[test]
    fn minter_respects_config_ttl_when_no_override() {
        // TTL=90s -> claim.exp ≈ now + 90.
        let minter = make_minter(90);
        let v = make_validator();
        let tid = TenantId::new("public").unwrap();
        let (token, _) = minter.mint(&tid, None, None, None).expect("mint");
        let claims = v.validate(&token).expect("validate");
        let delta = claims.exp as i64 - claims.iat as i64;
        assert!(
            (85..=95).contains(&delta),
            "TTL must be ~90s, got delta={delta}"
        );
    }

    #[test]
    fn minter_honors_explicit_ttl_override() {
        let minter = make_minter(86_400);
        let v = make_validator();
        let tid = TenantId::new("public").unwrap();
        let (token, _) = minter.mint(&tid, None, None, Some(30)).expect("mint");
        let claims = v.validate(&token).expect("validate");
        let delta = claims.exp as i64 - claims.iat as i64;
        assert!((25..=35).contains(&delta), "override TTL must be 30s");
    }

    #[test]
    fn minter_without_principal_produces_token_without_principal_claim() {
        let minter = make_minter(300);
        let v = make_validator();
        let tid = TenantId::new("public").unwrap();
        let (token, _) = minter.mint(&tid, None, None, None).expect("mint");
        let claims = v.validate(&token).expect("validate");
        assert!(
            claims.principal_id.is_none(),
            "minting without principal_id must produce claim without it"
        );
    }

    #[test]
    fn minter_new_fails_on_empty_private_pem() {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<t>".into(),
            private_key_path: "<t>".into(),
            ttl_seconds: 100,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        };
        let err = JwtMinter::new(&cfg, b"").expect_err("empty pem must fail");
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn minter_new_fails_on_garbage_pem() {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<t>".into(),
            private_key_path: "<t>".into(),
            ttl_seconds: 100,
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        };
        let err = JwtMinter::new(&cfg, b"not a pem").expect_err("garbage must fail");
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn minter_debug_does_not_leak_key_material() {
        let minter = make_minter(300);
        let dbg = format!("{minter:?}");
        assert!(dbg.contains("RS256"));
        assert!(dbg.contains("ttl_seconds"));
        assert!(
            !dbg.contains("PRIVATE"),
            "Debug must not include PEM bytes: {dbg}"
        );
        assert!(
            !dbg.contains("BEGIN"),
            "Debug must not include BEGIN marker: {dbg}"
        );
    }
}
