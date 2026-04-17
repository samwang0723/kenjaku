//! Financial asset extraction from LLM answers.
//!
//! When the merged generate call produces its structured JSON output
//! (`message` + `assets` + `suggestions`), `assets` carries any
//! stocks or crypto tickers the answer referenced. Surfaced on
//! `SearchResponse.assets` so clients (future UI sidebar cards, asset
//! chip rendering, etc.) can act on them without re-parsing the
//! answer body.

use serde::{Deserialize, Serialize};

/// An asset mentioned as a primary subject in the answer.
///
/// "Primary subject" means the asset is central to the answer, not a
/// passing mention. The prompt instructs the model to include only
/// assets the user is likely asking about or would want to follow up
/// on — e.g. "AAPL rose 2%" → `[{symbol: "AAPL", type: Stock}]`, but
/// "typical portfolios include stocks like AAPL" does not extract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// Ticker symbol in the asset's canonical form. Upper-case for
    /// stocks (AAPL, MSFT, TSLA) and crypto (BTC, ETH, SOL).
    pub symbol: String,
    /// Asset category. Only two classes are supported today; extend
    /// deliberately if a new category ships (etf / commodity / etc.).
    #[serde(rename = "type")]
    pub asset_type: AssetType,
}

/// Supported asset categories. Intentionally narrow — expand only
/// when a concrete use case lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetType {
    /// US-listed equities (tickers like AAPL, MSFT, NVDA).
    Stock,
    /// Cryptocurrencies (tickers like BTC, ETH, SOL).
    Crypto,
}

impl AssetType {
    pub fn as_str(self) -> &'static str {
        match self {
            AssetType::Stock => "stock",
            AssetType::Crypto => "crypto",
        }
    }

    /// Parse from the string the LLM produced. Tolerates common
    /// variants; anything off-list is rejected (caller drops the
    /// asset entry rather than guessing).
    pub fn from_raw(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "stock" | "equity" | "stocks" => Some(AssetType::Stock),
            "crypto" | "cryptocurrency" | "cryptos" => Some(AssetType::Crypto),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_type_from_raw() {
        assert_eq!(AssetType::from_raw("stock"), Some(AssetType::Stock));
        assert_eq!(AssetType::from_raw("Stock"), Some(AssetType::Stock));
        assert_eq!(AssetType::from_raw("  STOCK  "), Some(AssetType::Stock));
        assert_eq!(AssetType::from_raw("equity"), Some(AssetType::Stock));
        assert_eq!(AssetType::from_raw("crypto"), Some(AssetType::Crypto));
        assert_eq!(AssetType::from_raw("cryptocurrency"), Some(AssetType::Crypto));
        assert_eq!(AssetType::from_raw("etf"), None);
        assert_eq!(AssetType::from_raw(""), None);
    }

    #[test]
    fn asset_serializes_with_type_key() {
        let a = Asset {
            symbol: "AAPL".into(),
            asset_type: AssetType::Stock,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["symbol"], "AAPL");
        assert_eq!(v["type"], "stock");
    }

    #[test]
    fn asset_deserializes_from_wire_form() {
        let j = r#"{"symbol": "BTC", "type": "crypto"}"#;
        let a: Asset = serde_json::from_str(j).unwrap();
        assert_eq!(a.symbol, "BTC");
        assert_eq!(a.asset_type, AssetType::Crypto);
    }
}
