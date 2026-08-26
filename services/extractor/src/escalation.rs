//! The deterministic browser-escalation policy.
//!
//! Every render command the extractor publishes passes through [`decide`]. The
//! function is pure: it reads only facts the pipeline already established plus
//! configuration, and returns either permission to escalate or the first gate
//! that denied, in a fixed evaluation order pinned by tests.

use extractor_core::RenderConfig;

/// Facts established by the pipeline before the policy evaluates.
#[derive(Debug)]
pub(crate) struct EscalationInputs<'a> {
    /// A direct extraction rejected its content on quality grounds.
    pub quality_rejected: bool,
    /// Raw bytes carry empty-shell evidence with near-zero extracted text.
    pub empty_shell_evidence: bool,
    /// Host of the final URL; `None` when it cannot be determined.
    pub host: Option<&'a str>,
    /// Render configuration carrying the master switch and host allowlist.
    pub config: &'a RenderConfig,
    /// The per-UTC-day budget still has capacity for one more escalation.
    pub budget_remaining: bool,
}

/// Why the policy denied an escalation, in evaluation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscalationDenial {
    /// The direct attempt did not reject its content on quality.
    QualityNotRejected,
    /// Rendering is switched off.
    RenderingDisabled,
    /// The raw bytes are not empty-shell evidence.
    NotAnEmptyShell,
    /// A non-empty allowlist does not contain the target host.
    HostNotAllowed,
    /// The per-day budget has no remaining capacity.
    DailyBudgetExhausted,
}

impl EscalationDenial {
    /// Stable reason fragment recorded on the run beside the quality rejection.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::QualityNotRejected => "quality_not_rejected",
            Self::RenderingDisabled => "rendering_disabled",
            Self::NotAnEmptyShell => "not_an_empty_shell",
            Self::HostNotAllowed => "host_not_allowed",
            Self::DailyBudgetExhausted => "daily_budget_exhausted",
        }
    }
}

/// The outcome of one policy evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscalationDecision {
    /// Every gate permitted; publish exactly one render command.
    Escalate,
    /// The named gate denied; publish nothing.
    Denied(EscalationDenial),
}

/// Hosts compare case-insensitively and exactly; an empty allowlist imposes no
/// restriction beyond the other gates.
fn host_allowed(host: Option<&str>, allowed_hosts: &[String]) -> bool {
    let Some(host) = host else {
        return allowed_hosts.is_empty();
    };
    if allowed_hosts.is_empty() {
        return true;
    }
    let lowered = host.to_ascii_lowercase();
    allowed_hosts.iter().any(|allowed| allowed == &lowered)
}

/// Evaluates every gate in one fixed order: quality rejection, master switch,
/// shell evidence, host allowlist, daily budget. Any failing gate denies.
#[must_use]
pub(crate) fn decide(inputs: &EscalationInputs<'_>) -> EscalationDecision {
    if !inputs.quality_rejected {
        return EscalationDecision::Denied(EscalationDenial::QualityNotRejected);
    }
    if !inputs.config.enabled {
        return EscalationDecision::Denied(EscalationDenial::RenderingDisabled);
    }
    if !inputs.empty_shell_evidence {
        return EscalationDecision::Denied(EscalationDenial::NotAnEmptyShell);
    }
    if !host_allowed(inputs.host, &inputs.config.allowed_hosts) {
        return EscalationDecision::Denied(EscalationDenial::HostNotAllowed);
    }
    if !inputs.budget_remaining {
        return EscalationDecision::Denied(EscalationDenial::DailyBudgetExhausted);
    }
    EscalationDecision::Escalate
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use extractor_core::ExtractorConfig;

    /// One gate vector under test, encoded as one bit per gate.
    #[derive(Debug, Clone, Copy)]
    struct GateMask(u8);

    impl GateMask {
        const fn quality_rejected(self) -> bool {
            self.0 & 1 != 0
        }

        const fn enabled(self) -> bool {
            self.0 & 2 != 0
        }

        const fn shell(self) -> bool {
            self.0 & 4 != 0
        }

        const fn budget_remaining(self) -> bool {
            self.0 & 8 != 0
        }
    }

    /// Independent restatement of the specified behaviour used to check the
    /// implementation across every input combination.
    fn expected(gates: GateMask, host_allowed: bool) -> EscalationDecision {
        if !gates.quality_rejected() {
            return EscalationDecision::Denied(EscalationDenial::QualityNotRejected);
        }
        if !gates.enabled() {
            return EscalationDecision::Denied(EscalationDenial::RenderingDisabled);
        }
        if !gates.shell() {
            return EscalationDecision::Denied(EscalationDenial::NotAnEmptyShell);
        }
        if !host_allowed {
            return EscalationDecision::Denied(EscalationDenial::HostNotAllowed);
        }
        if !gates.budget_remaining() {
            return EscalationDecision::Denied(EscalationDenial::DailyBudgetExhausted);
        }
        EscalationDecision::Escalate
    }

    fn render_config(enabled: bool, allowed_hosts: &[&str]) -> RenderConfig {
        let mut config =
            ExtractorConfig::built_in(Path::new("/var/lib/ratatoskr-extractor/blobs")).render;
        config.enabled = enabled;
        config.allowed_hosts = allowed_hosts
            .iter()
            .map(|host| (*host).to_owned())
            .collect();
        config
    }

    #[test]
    fn every_gate_combination_resolves_to_the_specified_outcome() {
        let unrestricted = render_config(true, &[]);
        let allowlisted = render_config(true, &["example.com"]);
        let disabled = render_config(false, &[]);

        // Host dimension: any host under no allowlist, a case-variant of a listed
        // host, and an unlisted host under a non-empty allowlist.
        let host_cases: [(Option<&str>, &RenderConfig, bool); 3] = [
            (Some("any.host.example"), &unrestricted, true),
            (Some("Example.COM"), &allowlisted, true),
            (Some("other.com"), &allowlisted, false),
        ];

        // 2 x 2 x 2 x 3 x 2 exhaustive combinations decoded from one index so the
        // enumeration itself stays flat.
        for index in 0..48_u8 {
            let gates = GateMask(index & 0b1111);
            let (host, host_config, host_ok) = host_cases[usize::from((index >> 4) & 0b11)];
            let active_config = if gates.enabled() {
                host_config
            } else {
                &disabled
            };
            let outcome = decide(&EscalationInputs {
                quality_rejected: gates.quality_rejected(),
                empty_shell_evidence: gates.shell(),
                host,
                config: active_config,
                budget_remaining: gates.budget_remaining(),
            });
            let wanted = expected(gates, host_ok);
            assert_eq!(
                outcome, wanted,
                "index {index} decodes to {gates:?} with host {host:?}"
            );
        }
    }

    #[test]
    fn rendering_disabled_denies_regardless_of_every_other_permit() {
        let config = render_config(false, &[]);
        let decision = decide(&EscalationInputs {
            quality_rejected: true,
            empty_shell_evidence: true,
            host: Some("example.com"),
            config: &config,
            budget_remaining: true,
        });
        assert_eq!(
            decision,
            EscalationDecision::Denied(EscalationDenial::RenderingDisabled)
        );
    }

    #[test]
    fn missing_host_denies_only_under_a_nonempty_allowlist() {
        let unrestricted = render_config(true, &[]);
        let allowlisted = render_config(true, &["example.com"]);
        let permitted = |config: &RenderConfig| {
            decide(&EscalationInputs {
                quality_rejected: true,
                empty_shell_evidence: true,
                host: None,
                config,
                budget_remaining: true,
            })
        };
        assert_eq!(permitted(&unrestricted), EscalationDecision::Escalate);
        assert_eq!(
            permitted(&allowlisted),
            EscalationDecision::Denied(EscalationDenial::HostNotAllowed)
        );
    }

    #[test]
    fn denial_reasons_are_stable_strings() {
        assert_eq!(
            EscalationDenial::HostNotAllowed.as_str(),
            "host_not_allowed"
        );
        assert_eq!(
            EscalationDenial::DailyBudgetExhausted.as_str(),
            "daily_budget_exhausted"
        );
    }
}
