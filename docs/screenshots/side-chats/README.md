# Side chats on iOS

Native captures from the demo workspace (offline, real Rust core, simulated host —
no network or signed-in account). They cover the P1 browser and the P2 creation
paths: the session menu and sheet entry points, an empty side chat, a fork
through the last completed response, the transcript's fork seam, and search.

```sh
./scripts/ios/test-side-chats.sh <simulator-udid> [output-dir]
```

The script runs the five `ZeronUITests/SessionFlowTests` cases below and exports
their `XCTAttachment` screenshots from the result bundle. Captured on iPhone 17
Pro and iPad Pro 11-inch (M5), iOS 26 simulator.

Captured flows:

- `session-menu.png`: the session's ⋯ menu with **New Side Chat** and **Fork to
  Side Chat**, plus the existing Pin / Copy Transcript / Archive actions.
- `side-chats-sheet.png`: the branch button (count and live tone) opens the
  sheet; rows use the standard session cell, opening one through the shell.
- `new-side-chat.png`: `+` mints an empty child under the parent — same host,
  project and model — and opens it ready for the first message. Its subtitle
  names the parent and taps back to it; the menu's fork action is disabled
  until a completed reply exists.
- `forked-side-chat.png`: **Fork to Side Chat** copies the source through its
  latest completed response; the copy opens with the seam and the parent in
  its title.
- `fork-seam.png`: the "Forked from …" seam is a `zeron://chat/<id>` link that
  jumps back to the source chat.
- `search-side-chats.png`: search finds side chats the front page never lists
  and groups them under their own **Side chats** header.
- `side-chat-archived.png`: archiving a child from the sheet drops its row and
  the parent's branch-button count live (child rows never enter the page
  lists, so the workspace revision drives the refresh).
- `ipad-popover.png`: the same sheet as a popover beside the iPad sidebar.

Tests: `testSideChatsSheetAndForkSeam`, `testNewSideChatFromMenu`,
`testForkToSideChatFromMenu`, `testArchivingSideChatUpdatesParent`,
`testSearchFindsSideChats`.

Notes: in demo mode `ForkSideChat` is the in-process equivalent of the engine
RPC the live app sends to the chat's owning host; these captures validate the
UI and navigation, not device synchronization. The child's opening spinner now
clears as soon as the empty session is hydrated.
