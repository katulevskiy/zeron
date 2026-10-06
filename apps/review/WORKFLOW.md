# Pilot contribution workflow

Proposed policy for maintainer review. Capability names in the app do not grant GitHub repository permissions.

## Authority

Maintainers set direction, nominate trusted contributors, approve high-risk revisions, and initially retain final merge authority. Triagers classify scope, component, priority, risk, and required platform coverage. They may accept low-risk changes consistent with established direction. Medium/high-risk direction decisions require a maintainer. Reviewers assess implementation. Validators record concrete behavior tests, platform/build information, results, and artifact links.

Any signed-in contributor can claim a required task or express demand/urgency with a use case. A claim coordinates work; it does not grant approval authority. Authors cannot claim independent review/validation tasks on their own changes or attest to readiness. Agents act under an accountable contributor's identity. Review and validation can run in parallel; multiple tokens for one person remain one principal.

## Evidence and flow

Direction accepted → required triage complete → independent code review → required behavior/platform validation → required CI → final merge decision. The app derives the stage from these records rather than allowing an arbitrary "verified" toggle. High-risk revisions also require explicit maintainer risk approval.

An outstanding trusted review requesting changes or a current failed platform validation blocks readiness, even if someone else approves. A reviewer/validator can resolve their own finding with an updated record. Native dismissed reviews no longer contribute approval. Capability revocation removes the approval's authority on the next evaluation.

Evidence belongs to a revision. New code invalidates approvals and validations for readiness while preserving the old history; claims expire on new revisions. Claims otherwise have renewable leases. Required platform coverage and risk cannot be weakened after triage without a maintainer. Native integration checks must establish compatibility with the current base before merging.

Low-risk work includes routine documentation or narrowly scoped fixes under an established design. Broad behavior changes are medium risk by default. Authentication, sync semantics, data migrations, and architectural changes require explicit high-risk consideration. Actual classifications remain maintainer decisions.

## Response and disposition

Aim to triage incoming work within three working days and acknowledge accepted review work within a week. These are pilot targets, not service guarantees. Keep active claims within reviewer capacity; prefer finishing a review to accumulating claims.

No PR should pass 30 days without a reasoned disposition: merge, reject with explanation, request specific author changes, defer with a linked backlog issue, or document an active exception with an owner and next decision date. Waiting for the review team must not be treated as author inactivity. There is no automatic closure.

Issues are the canonical problem/feature backlog. Triage reproducibility, impact, affected area/platform, priority, and Now/Next/Later placement. Link competing implementations and duplicates on GitHub. For substantial features, accept scope on an issue before implementation.

## Recognition and delegation

Recognize discovery, reproduction, design, implementation, triage, review, validation, documentation, and support. Roles describe different work; they are not one numeric ladder. Grant trust by area and demonstrated judgment. Inspect actual contributions and nominate capabilities explicitly rather than awarding write access by PR count or votes.

After the pilot, delegate routine merges to trusted contributors where branch rules and area ownership allow it. Keep product/architecture and high-risk decisions with designated owners. The purpose of the final review packet is to let the maintainer rely on trusted evidence and inspect unresolved concerns.
