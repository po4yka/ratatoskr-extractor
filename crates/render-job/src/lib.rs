#![forbid(unsafe_code)]

//! Wire types implementing the stored Ratatoskr browser-rendering contract.

use ratatoskr_identifiers::BlobRef;

/// Subject carrying render commands from the extractor to the browser worker.
pub const RENDER_REQUESTED_SUBJECT: &str = "cmd.content.render.requested.v1";
/// Subject carrying successful render evidence.
pub const RENDER_COMPLETED_SUBJECT: &str = "evt.content.render.completed.v1";
/// Subject carrying terminal render failures.
pub const RENDER_FAILED_SUBJECT: &str = "evt.content.render.failed.v1";

/// Why a render job produced no document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderFailureClass {
    /// A navigation target or redirect hop violated the SSRF policy.
    PolicyBlocked,
    /// Navigation did not settle within its budget.
    NavigationTimeout,
    /// The whole job exceeded its total budget.
    TotalTimeout,
    /// The rendered document exceeded the DOM size budget.
    SizeLimit,
    /// Navigation failed at the transport level.
    NavigationFailed,
    /// Chromium could not be reached or launched.
    BrowserUnavailable,
}

impl RenderFailureClass {
    /// Stable wire spelling of the class.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyBlocked => "policy_blocked",
            Self::NavigationTimeout => "navigation_timeout",
            Self::TotalTimeout => "total_timeout",
            Self::SizeLimit => "size_limit",
            Self::NavigationFailed => "navigation_failed",
            Self::BrowserUnavailable => "browser_unavailable",
        }
    }
}

/// Finite budgets a render command carries for one job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenderBudgets {
    /// Navigation deadline in milliseconds.
    pub navigation_timeout_ms: u64,
    /// Whole-job deadline in milliseconds.
    pub total_timeout_ms: u64,
    /// Maximum rendered DOM bytes accepted as evidence.
    pub max_dom_bytes: usize,
}

/// One render request. Unknown fields are rejected so callers cannot express credentials.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderCommand {
    /// Unique job identity; the deduplication key.
    pub render_id: uuid::Uuid,
    /// Operation owning the requesting run.
    pub operation_id: uuid::Uuid,
    /// Correlation identifier propagated into worker events.
    pub correlation_id: String,
    /// Owning user identity of the requesting operation.
    pub tenant_user_id: uuid::Uuid,
    /// Target URL; the worker revalidates it before navigating.
    pub url: String,
    /// Job budgets.
    pub budgets: RenderBudgets,
}

/// One observed navigation hop.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RedirectHop {
    /// Hop URL.
    pub url: String,
    /// Response status.
    pub status: u16,
    /// Declared media type when known.
    pub media_type: Option<String>,
}

/// Network-evidence summary published with completed jobs; never carries bodies.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetworkEvidence {
    /// Observed hops in order, starting at the requested target.
    pub hops: Vec<RedirectHop>,
    /// Heavy subresource requests denied by interception.
    pub blocked_requests: u64,
}

/// Successful completion announcing worker-owned rendered DOM.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenderCompleted {
    /// Job identity this completion answers.
    pub render_id: uuid::Uuid,
    /// Final URL after all hops.
    pub final_url: String,
    /// Worker-owned reference to the rendered DOM bytes.
    pub dom: BlobRef,
    /// Network-evidence summary.
    pub evidence: NetworkEvidence,
}

/// Terminal failure of a render job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenderFailed {
    /// Job identity this failure answers.
    pub render_id: uuid::Uuid,
    /// Stable failure class carried to the extractor's terminal record.
    pub class: RenderFailureClass,
}
