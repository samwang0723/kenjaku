use serde::{Deserialize, Serialize};

/// Classified intent of a user query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Factual question seeking specific information.
    Factual,
    /// Navigational query looking for a specific page or resource.
    Navigational,
    /// How-to or procedural question.
    HowTo,
    /// Comparison or evaluation between options.
    Comparison,
    /// Troubleshooting or debugging a problem.
    Troubleshooting,
    /// Exploratory or open-ended research question.
    Exploratory,
    /// Conversational / chitchat (not a real search).
    Conversational,
    /// Could not determine intent.
    Unknown,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Factual => "factual",
            Intent::Navigational => "navigational",
            Intent::HowTo => "how_to",
            Intent::Comparison => "comparison",
            Intent::Troubleshooting => "troubleshooting",
            Intent::Exploratory => "exploratory",
            Intent::Conversational => "conversational",
            Intent::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Result of intent classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentClassification {
    pub intent: Intent,
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_display() {
        assert_eq!(Intent::Factual.to_string(), "factual");
        assert_eq!(Intent::HowTo.to_string(), "how_to");
        assert_eq!(Intent::Troubleshooting.to_string(), "troubleshooting");
    }

    #[test]
    fn test_intent_serde_roundtrip() {
        let intent = Intent::Comparison;
        let json = serde_json::to_string(&intent).unwrap();
        assert_eq!(json, "\"comparison\"");
        let parsed: Intent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Intent::Comparison);
    }

    #[test]
    fn test_intent_classification() {
        let classification = IntentClassification {
            intent: Intent::Factual,
            confidence: 0.92,
        };
        assert_eq!(classification.intent, Intent::Factual);
        assert!(classification.confidence > 0.9);
    }
}
