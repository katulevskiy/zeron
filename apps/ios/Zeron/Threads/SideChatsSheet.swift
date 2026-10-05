import UIKit

/// What an error alert shows for a core failure: the payload, not the
/// reflected case name.
func sideChatErrorMessage(_ error: Error) -> String {
    guard let core = error as? CoreError else { return error.localizedDescription }
    switch core {
    case let .NotFound(message), let .InvalidArgument(message), let .HostUnavailable(message),
         let .Unsupported(message), let .HostError(message), let .Network(message),
         let .Auth(message), let .Storage(message), let .NotImplemented(message),
         let .Internal(message):
        return message
    case .Closed:
        return "Not signed in."
    }
}

/// A chat's side chats (forks and agent-spawned children) as a sheet list.
/// Rows are the same `SessionCell` the main list uses; tapping one opens it
/// through the shell's router. The header creates more: "+" mints an empty
/// child, the branch button forks through the latest completed response.
final class SideChatsSheetController: SessionListController {
    private let parentId: String
    private let empty = UILabel()
    private let busyIndicator = UIActivityIndicatorView(style: .medium)
    private var busy = false {
        didSet { updateActions() }
    }
    private lazy var doneItem = UIBarButtonItem(
        title: "Close",
        style: .plain,
        target: self,
        action: #selector(closeTapped)
    )
    private lazy var newItem = UIBarButtonItem(
        systemItem: .add,
        primaryAction: UIAction { [weak self] _ in self?.newSideChat() }
    )
    private lazy var forkItem = UIBarButtonItem(
        image: UIImage(systemName: "arrow.triangle.branch"),
        primaryAction: UIAction { [weak self] _ in self?.forkFromLastResponse() }
    )
    private lazy var spinnerItem = UIBarButtonItem(customView: busyIndicator)

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
        newItem.accessibilityLabel = "New side chat"
        newItem.accessibilityIdentifier = "side-chats-new"
        forkItem.accessibilityLabel = "Fork from last response"
        forkItem.accessibilityIdentifier = "side-chats-fork"
        doneItem.accessibilityIdentifier = "side-chats-close"
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
        navigationItem.leftBarButtonItem = doneItem
        updateActions()
    }

    @objc private func closeTapped() {
        dismiss(animated: true)
    }

    private func updateActions() {
        if busy {
            navigationItem.rightBarButtonItems = [spinnerItem]
        } else {
            // Same completed-response boundary as the engine: a working tail
            // may still fork the last complete reply.
            let canFork = app.canFork(parentId)
            forkItem.isEnabled = canFork
            forkItem.accessibilityHint = canFork ? nil : "Needs a completed reply"
            navigationItem.rightBarButtonItems = [newItem, forkItem]
        }
        if busy {
            busyIndicator.startAnimating()
        } else {
            busyIndicator.stopAnimating()
        }
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("side-chats", nil, [], app.children(of: parentId))]
    }

    override func reload(animated: Bool) {
        super.reload(animated: animated)
        empty.isHidden = !sessions.isEmpty
    }

    /// A side chat opens in place of the sheet: dismiss first so the shell's
    /// push (iPhone, keeping the parent below) or column swap (iPad) isn't
    /// covered by it. The router is captured before dismissal — by the
    /// completion the sheet has left the window, and `view.window` alone
    /// would resolve to nil.
    override func openSession(_ id: String) {
        let router = self.router
        if presentingViewController != nil {
            dismiss(animated: true) { _ = router?.openChildSession(id) }
        } else {
            router?.openChildSession(id)
        }
    }

    private func newSideChat() {
        do {
            openSession(try app.createSideChat(parentId: parentId))
        } catch {
            presentError("Couldn't create a side chat", error)
        }
    }

    private func forkFromLastResponse() {
        guard !busy, app.canFork(parentId) else { return }
        busy = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let id = try await app.forkSideChat(sourceId: parentId)
                busy = false
                openSession(id)
            } catch {
                busy = false
                presentError("Couldn't fork this chat", error)
            }
        }
    }

    private func presentError(_ title: String, _ error: Error) {
        let alert = UIAlertController(title: title, message: sideChatErrorMessage(error), preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default))
        present(alert, animated: true)
    }
}
