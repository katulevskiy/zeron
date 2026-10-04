import UIKit

/// A chat's side chats (forks and agent-spawned children) as a sheet list.
/// Rows are the same `SessionCell` the main list uses; tapping one opens it
/// through the shell's router. Presented by `SessionViewController`'s branch
/// button — side chats never list on the front page.
final class SideChatsSheetController: SessionListController {
    private let parentId: String
    private let empty = UILabel()

    init(app: AppModel, parentId: String, currentChatId: String?) {
        self.parentId = parentId
        super.init(app: app)
        self.currentChatId = currentChatId
        compactActions = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Side chats"
        empty.text = "No side chats yet."
        empty.font = Fonts.ui(.sans, 15)
        empty.textColor = Palette.secondary
        empty.textAlignment = .center
        empty.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(empty)
        NSLayoutConstraint.activate([
            empty.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: view.centerYAnchor, constant: -40),
            empty.leadingAnchor.constraint(greaterThanOrEqualTo: view.leadingAnchor, constant: 24),
        ])
        empty.isHidden = !sessions.isEmpty
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("side-chats", nil, [], app.children(of: parentId))]
    }

    override func reload(animated: Bool) {
        super.reload(animated: animated)
        empty.isHidden = !sessions.isEmpty
    }

    /// A side chat opens in place of the sheet: dismiss first so the shell's
    /// push (iPhone) or column swap (iPad) isn't covered by it. The router is
    /// captured before dismissal — by the completion the sheet has left the
    /// window, and `view.window` alone would resolve to nil.
    override func openSession(_ id: String) {
        let router = self.router
        if presentingViewController != nil {
            dismiss(animated: true) { _ = router?.openSession(id) }
        } else {
            router?.openSession(id)
        }
    }
}
