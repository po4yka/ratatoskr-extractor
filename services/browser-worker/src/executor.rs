//! Chromium-backed render execution with isolation, interception, and budgets.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams, RequestPattern,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, EventResponseReceived};
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams, DisposeBrowserContextParams,
};
use chromiumoxide::{BrowserConfig, Page};
use extractor_url_routing::{
    RoutingPolicy, SystemDnsLookup, ValidatingResolver, normalize, validate_address,
};
use futures_util::StreamExt as _;
use render_job::{NetworkEvidence, RedirectHop, RenderCommand, RenderFailureClass};

use crate::{RenderOutcome, WorkerError};

/// Navigation policy for the worker: shared SSRF rules plus a test-only loopback escape.
#[derive(Debug, Clone)]
pub struct NavigationPolicy {
    /// URL and port rules applied to every navigation hop.
    pub routing: RoutingPolicy,
    /// Permit `localhost`/loopback targets; integration tests only.
    pub allow_loopback: bool,
}

impl Default for NavigationPolicy {
    fn default() -> Self {
        Self {
            routing: RoutingPolicy {
                max_url_length: 8_192,
                allowed_ports: vec![80, 443],
            },
            allow_loopback: false,
        }
    }
}

/// Renders pages through a shared Chromium process, one isolated context per job.
#[derive(Debug)]
pub struct ChromiumExecutor {
    browser: tokio::sync::Mutex<Browser>,
    policy: NavigationPolicy,
}

/// Why the executor could not start or complete a rendering.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    /// Chromium failed to launch or answer.
    #[error("chromium is unavailable")]
    BrowserUnavailable(#[from] chromiumoxide::error::CdpError),
    /// A budget was exceeded; the class names which one.
    #[error("render exceeded its budget")]
    Budget(RenderFailureClass),
    /// A navigation destination violated the SSRF policy.
    #[error("navigation target violates the SSRF policy")]
    PolicyBlocked,
    /// Launch configuration was rejected.
    #[error("chromium launch configuration is invalid: {0}")]
    InvalidLaunchConfig(String),
}

impl From<ExecutorError> for WorkerError {
    fn from(error: ExecutorError) -> Self {
        match error {
            ExecutorError::BrowserUnavailable(source) => {
                tracing::warn!(error = %source, "chromium failed");
                WorkerError::Failed(RenderFailureClass::BrowserUnavailable)
            }
            ExecutorError::Budget(class) => WorkerError::Failed(class),
            ExecutorError::PolicyBlocked => WorkerError::Failed(RenderFailureClass::PolicyBlocked),
            ExecutorError::InvalidLaunchConfig(message) => {
                tracing::warn!(message = %message, "chromium launch configuration rejected");
                WorkerError::Failed(RenderFailureClass::BrowserUnavailable)
            }
        }
    }
}

fn infrastructure<E>(error: E) -> WorkerError
where
    E: std::error::Error + Send + Sync + 'static,
{
    WorkerError::Infrastructure(Box::new(error))
}

impl ChromiumExecutor {
    /// Launches one shared headless Chromium process under the production navigation policy.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorError`] when launch fails.
    pub fn launch(
        chrome_bin: Option<String>,
    ) -> impl std::future::Future<Output = Result<Self, ExecutorError>> + Send {
        Self::launch_with_policy(chrome_bin, NavigationPolicy::default())
    }

    /// Launches with an explicit navigation policy; integration tests allow loopback ports.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorError`] when launch fails.
    pub async fn launch_with_policy(
        chrome_bin: Option<String>,
        policy: NavigationPolicy,
    ) -> Result<Self, ExecutorError> {
        let data_dir = std::env::temp_dir().join(format!(
            "ratatoskr-browser-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| ExecutorError::InvalidLaunchConfig(error.to_string()))?;
        let mut builder = BrowserConfig::builder()
            .user_data_dir(&data_dir)
            .arg("--no-proxy-server")
            .arg("--no-first-run")
            .arg("--disable-gpu");
        if let Some(binary) = chrome_bin {
            builder = builder.chrome_executable(binary);
        }
        let config = builder
            .build()
            .map_err(ExecutorError::InvalidLaunchConfig)?;
        let (browser, mut handler) = Browser::launch(config).await?;
        tokio::spawn(async move {
            // Drives the CDP connection; dropping it would stall every command.
            while handler.next().await.is_some() {}
        });
        Ok(Self {
            browser: tokio::sync::Mutex::new(browser),
            policy,
        })
    }

    /// Renders one command target inside a fresh isolated browser context.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorError`] when the browser fails or a budget is exceeded.
    pub async fn render(&self, command: &RenderCommand) -> Result<RenderOutcome, ExecutorError> {
        let total = Duration::from_millis(command.budgets.total_timeout_ms);
        let mut browser = self.browser.lock().await;
        let outcome = tokio::time::timeout(total, render_once(&mut browser, command, &self.policy))
            .await
            .map_err(|_| ExecutorError::Budget(RenderFailureClass::TotalTimeout))??;
        Ok(outcome)
    }
}

/// Revalidates one navigation destination against the shared SSRF policy.
///
/// Returns `false` when the policy forbids the hop.
async fn navigation_allowed(url: &str, policy: &NavigationPolicy) -> bool {
    let Ok(normalized) = normalize(url, &policy.routing) else {
        return false;
    };
    let Some(host) = normalized.normalized().host_str().map(str::to_owned) else {
        return false;
    };
    if policy.allow_loopback && matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return true;
    }
    let resolver = ValidatingResolver::new(SystemDnsLookup);
    match resolver.resolve_host(&host).await {
        Ok(addresses) => addresses
            .iter()
            .all(|address| validate_address(address.ip()).is_ok()),
        Err(_) => false,
    }
}

async fn render_once(
    browser: &mut Browser,
    command: &RenderCommand,
    policy: &NavigationPolicy,
) -> Result<RenderOutcome, ExecutorError> {
    if !navigation_allowed(&command.url, policy).await {
        return Err(ExecutorError::PolicyBlocked);
    }
    let context_id = browser
        .execute(CreateBrowserContextParams::builder().build())
        .await?
        .result
        .browser_context_id;
    // Disposing the context destroys every page inside it however the job ended.
    let result = render_in_context(browser, command, policy, context_id.clone()).await;
    browser
        .execute(DisposeBrowserContextParams::new(context_id))
        .await
        .ok();
    result
}

async fn render_in_context(
    browser: &mut Browser,
    command: &RenderCommand,
    policy: &NavigationPolicy,
    context_id: BrowserContextId,
) -> Result<RenderOutcome, ExecutorError> {
    // The page is created blank: the single navigation below is the only load of the job, so
    // the command URL is fetched exactly once and every request passes through interception.
    let target = CreateTargetParams::builder()
        .url("about:blank")
        .browser_context_id(context_id)
        .build()
        .map_err(ExecutorError::InvalidLaunchConfig)?;
    let page = browser.new_page(target).await?;
    // Interception is installed before the single navigation so every request of the job is
    // subject to policy validation and heavy-resource denial.
    let (blocked, policy_violated) = install_interception(&page, policy.clone())
        .await
        .map_err(|error| ExecutorError::InvalidLaunchConfig(error.to_string()))?;
    let hops_log: Arc<std::sync::Mutex<Vec<RedirectHop>>> = Arc::default();
    if let Ok(mut responses) = page.event_listener::<EventResponseReceived>().await {
        let hops_task = Arc::clone(&hops_log);
        tokio::spawn(async move {
            while let Some(event) = responses.next().await {
                if let Ok(mut hops) = hops_task.lock() {
                    hops.push(RedirectHop {
                        url: event.response.url.clone(),
                        status: u16::try_from(event.response.status).unwrap_or(0),
                        media_type: Some(event.response.mime_type.clone()),
                    });
                }
            }
        });
    }

    let navigation = Duration::from_millis(command.budgets.navigation_timeout_ms);
    tokio::time::timeout(navigation, async {
        page.goto(command.url.as_str()).await.ok();
        // Give hydration a short settle window instead of waiting forever on network idle.
        tokio::time::sleep(Duration::from_millis(750)).await;
    })
    .await
    .map_err(|_| ExecutorError::Budget(RenderFailureClass::NavigationTimeout))?;

    if policy_violated.load(Ordering::Relaxed) {
        return Err(ExecutorError::PolicyBlocked);
    }
    let dom = page
        .content()
        .await
        .map_err(ExecutorError::BrowserUnavailable)?;
    eprintln!(
        "DIAG-EXEC dom len={} budget={} head={:?}",
        dom.len(),
        command.budgets.max_dom_bytes,
        dom.chars().take(120).collect::<String>()
    );
    if dom.len() > command.budgets.max_dom_bytes {
        return Err(ExecutorError::Budget(RenderFailureClass::SizeLimit));
    }

    Ok(RenderOutcome {
        final_url: page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| command.url.clone()),
        dom: dom.into_bytes(),
        evidence: NetworkEvidence {
            hops: hops_log.lock().map(|hops| hops.clone()).unwrap_or_default(),
            blocked_requests: blocked.load(Ordering::Relaxed),
        },
    })
}

pub(crate) async fn install_interception(
    page: &Page,
    policy: NavigationPolicy,
) -> Result<(Arc<AtomicU64>, Arc<AtomicBool>), WorkerError> {
    let blocked = Arc::new(AtomicU64::new(0));
    let violated = Arc::new(AtomicBool::new(false));

    // The listener is registered before the domain is enabled so the first paused request is
    // never lost.
    if let Ok(mut failed) = page
        .event_listener::<chromiumoxide::cdp::browser_protocol::network::EventLoadingFailed>()
        .await
    {
        tokio::spawn(async move {
            while let Some(event) = failed.next().await {
                eprintln!(
                    "DIAG-EXEC load failed: {:?} canceled={:?}",
                    event.error_text, event.canceled
                );
            }
        });
    }
    let mut paused = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|error| WorkerError::Infrastructure(Box::new(error)))?;
    let patterns = vec![
        RequestPattern::builder()
            .url_pattern("*")
            .request_stage(chromiumoxide::cdp::browser_protocol::fetch::RequestStage::Request)
            .build(),
    ];
    let enable = EnableParams::builder().patterns(patterns).build();
    page.execute(enable).await.map_err(infrastructure)?;

    let page_handle = page.clone();
    let blocked_task = Arc::clone(&blocked);
    let violated_task = Arc::clone(&violated);
    tokio::spawn(async move {
        while let Some(event) = paused.next().await {
            if !navigation_allowed(&event.request.url, &policy).await {
                deny_request(&page_handle, &event).await;
                violated_task.store(true, Ordering::Relaxed);
                continue;
            }
            if is_heavy(&event) && deny(&page_handle, &event).await {
                blocked_task.fetch_add(1, Ordering::Relaxed);
            }
            let _ = page_handle
                .execute(ContinueRequestParams::new(event.request_id.clone()))
                .await;
        }
    });
    Ok((blocked, violated))
}

async fn deny_request(page: &Page, event: &EventRequestPaused) {
    let denied = deny(page, event).await;
    let _ = denied;
}

async fn deny(page: &Page, event: &EventRequestPaused) -> bool {
    let Ok(fail) = FailRequestParams::builder()
        .request_id(event.request_id.clone())
        .error_reason(ErrorReason::BlockedByClient)
        .build()
    else {
        return false;
    };
    page.execute(fail).await.is_ok()
}

fn is_heavy(event: &EventRequestPaused) -> bool {
    use chromiumoxide::cdp::browser_protocol::network::ResourceType;
    matches!(
        event.resource_type,
        ResourceType::Image | ResourceType::Font | ResourceType::Media
    )
}
