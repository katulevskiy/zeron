// DO NOT EDIT — generated from upstream Rust by `cargo run -p wiregen` (check: `-- --check`).

/**
 * A user-named sidebar section. Archived sessions retain membership so restoring
 * them restores their section; deleting the section never deletes sessions.
 */
export type SidebarSection = { id: string, name: string, session_ids: Array<string>, collapsed: boolean, };
