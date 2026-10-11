//! Summing [`AgentEvent::Usage`] reports. Consumers add up every `Usage` a
//! turn emits, so drivers that learn a turn's usage in several pieces (one
//! per model request or assistant message) fold them into one report first.
use zeron_proto::AgentEvent;

fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

/// `a + b` for two `Usage` events. Optional fields stay `None` only when
/// neither side reported them. Non-`Usage` events return `a` unchanged.
pub(crate) fn sum(a: AgentEvent, b: &AgentEvent) -> AgentEvent {
    let (
        AgentEvent::Usage {
            input_tokens: ai,
            output_tokens: ao,
            cache_read_tokens: acr,
            cache_write_tokens: acw,
            reasoning_tokens: ar,
            cost_usd: ac,
        },
        AgentEvent::Usage {
            input_tokens: bi,
            output_tokens: bo,
            cache_read_tokens: bcr,
            cache_write_tokens: bcw,
            reasoning_tokens: br,
            cost_usd: bc,
        },
    ) = (&a, b)
    else {
        return a;
    };
    AgentEvent::usage(ai.saturating_add(*bi), ao.saturating_add(*bo))
        .with_cache(add(*acr, *bcr), add(*acw, *bcw))
        .with_reasoning(add(*ar, *br))
        .with_cost_usd(match (ac, bc) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        })
}

/// Folds `next` into an optional running total.
pub(crate) fn accumulate(total: &mut Option<AgentEvent>, next: AgentEvent) {
    *total = Some(match total.take() {
        Some(prev) => sum(prev, &next),
        None => next,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_counts_and_keeps_unknown_fields_unknown() {
        let a = AgentEvent::usage(10, 2)
            .with_cache(Some(5), None)
            .with_cost_usd(Some(0.5));
        let b = AgentEvent::usage(1, 3).with_reasoning(Some(4));
        assert_eq!(
            sum(a, &b),
            AgentEvent::usage(11, 5)
                .with_cache(Some(5), None)
                .with_reasoning(Some(4))
                .with_cost_usd(Some(0.5))
        );
        let mut total = None;
        accumulate(&mut total, AgentEvent::usage(1, 1));
        accumulate(&mut total, AgentEvent::usage(2, 2));
        assert_eq!(total, Some(AgentEvent::usage(3, 3)));
    }
}
