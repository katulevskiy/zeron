//! Moving a live chat between devices (plan: docs/plans/2026-09-30-agent-mobility-and-policy.md,
//! Part 1). The pure, local pieces: which files make up a workspace
//! ([`scope`]) and how its git state is captured on the source and landed on
//! the target ([`git`]).

pub mod git;
pub mod scope;
