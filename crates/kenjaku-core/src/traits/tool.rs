use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::types::tool::{ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest};

/// A pluggable external tool. Implementations live in
/// `kenjaku-service::tools/` (wrapping infra clients and domain logic).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier for this tool.
    fn id(&self) -> ToolId;

    /// Shared config (enabled flag, rollout pct).
    fn config(&self) -> &ToolConfig;

    /// Declare tool dependencies by ID. Default: empty (runs in Tier 0).
    /// The `ToolTunnel` resolves execution tiers via topological sort at
    /// construction time. Cycles panic.
    fn depends_on(&self) -> Vec<ToolId> {
        vec![]
    }

    /// Self-gating. `prior` contains accumulated outputs from all tools
    /// in earlier tiers. Tools with no deps get an empty map.
    fn should_fire(&self, req: &ToolRequest, prior: &ToolOutputMap) -> bool;

    /// Execute. `prior` contains accumulated outputs from dependency tiers.
    /// A dependent tool can read another tool's result.
    /// MUST honor `cancel.is_cancelled()` at every I/O await point.
    /// Return `ToolError::Cancelled` on cooperative cancel.
    async fn invoke(
        &self,
        req: &ToolRequest,
        prior: &ToolOutputMap,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError>;

    /// Render a `ToolOutput::Structured` payload into a text fact block
    /// the Brain can cite. Default impl serializes to pretty JSON.
    fn render_fact(&self, facts: &serde_json::Value) -> String {
        serde_json::to_string_pretty(facts).unwrap_or_default()
    }
}
