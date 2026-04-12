use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::types::tool::{ToolConfig, ToolError, ToolId, ToolOutput, ToolRequest};

/// A pluggable external tool. Implementations live in
/// `kenjaku-service::tools/` (wrapping infra clients and domain logic).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable identifier for this tool.
    fn id(&self) -> ToolId;

    /// Shared config (enabled flag, rollout pct).
    fn config(&self) -> &ToolConfig;

    /// Is this tool relevant for this request? Self-gating.
    /// Cheap, synchronous, no I/O. Implementations call
    /// `ToolConfig::should_fire_for(request_id)` first for the
    /// enabled/rollout check, then layer tool-specific logic.
    fn should_fire(&self, req: &ToolRequest, prior_chunk_count: usize) -> bool;

    /// Execute. MUST honor `cancel.is_cancelled()` at every I/O await
    /// point. Return `ToolError::Cancelled` on cooperative cancel.
    async fn invoke(
        &self,
        req: &ToolRequest,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError>;

    /// Render a `ToolOutput::Structured` payload into a text fact block
    /// the Brain can cite. Default impl serializes to pretty JSON.
    fn render_fact(&self, facts: &serde_json::Value) -> String {
        serde_json::to_string_pretty(facts).unwrap_or_default()
    }
}
