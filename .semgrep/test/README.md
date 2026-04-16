# Tenant-scope rule fixtures

Self-test fixtures for `../tenant-scope.yml`. These `.rs` files live
outside the Cargo workspace members so they are never compiled.

## Expected behavior

Run from the repository root:

```bash
semgrep --config .semgrep/tenant-scope.yml .semgrep/test/
```

Expected output: **exactly one finding**, in `bad_missing_tenant.rs`.

| Fixture | Expected | Why |
|---|---|---|
| `bad_missing_tenant.rs` | **FIRES** | Pre-H2 shape: `WHERE session_id = $1` with no tenant_id anywhere. |
| `good_with_tenant_in_sql.rs` | silent | `tenant_id` appears in the SQL string. |
| `good_with_tenant_bind.rs` | silent | `tenant_id` appears via `c.tenant_id` and `.bind(tctx_tenant_id)`. |

## Load-bearing test (H2 reconstruction)

The PM's non-negotiable acceptance criterion is that the rule fires on
the real pre-H2 state of `feedback.rs:get_by_session`. To reproduce:

```bash
git show 210fdfd~1:crates/kenjaku-infra/src/postgres/feedback.rs > /tmp/pre_h2_feedback.rs
semgrep --config .semgrep/tenant-scope.yml /tmp/pre_h2_feedback.rs
rm /tmp/pre_h2_feedback.rs
```

Expected: rule flags the `get_by_session` query that filters only by
`session_id`. Output captured in `.claude/tasks/3d-2-tenant-guardrails/dev-1.md`.
