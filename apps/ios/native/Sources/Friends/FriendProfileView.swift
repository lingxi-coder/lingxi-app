import SwiftUI

/// Profile settings (docs/FRIENDS-CLIENTS.md D11): the display name friends see, the username people can look up, and
/// whether they may find the viewer by username or email. One Save sends only what changed.
struct FriendProfileView: View {
    @Environment(FriendsWorkspace.self) private var friends

    /// The profile the form was filled from; `Save` compares against it.
    @State private var loaded: FriendProfile?
    @State private var name = ""
    @State private var handle = ""
    @State private var findByHandle = true
    @State private var findByEmail = true
    @State private var saving = false
    @State private var error = ""

    private var changes: [String: Any] {
        guard let loaded else { return [:] }
        return FriendsLogic.profileChanges(loaded, name: name, handle: handle, findByHandle: findByHandle, findByEmail: findByEmail)
    }
    /// An empty username means "leave as is": once set it can be changed but not removed.
    private var handleValid: Bool {
        let text = JSText.trim(handle)
        return text.isEmpty || FriendsLogic.isHandle(text)
    }
    private var canSave: Bool { loaded != nil && !saving && FriendsLogic.isDisplayName(name) && handleValid && !changes.isEmpty }

    var body: some View {
        Form {
            if loaded != nil {
                Section {
                    TextField("friends_profile_name", text: $name)
                        .accessibilityIdentifier("friends.profile.name")
                } header: {
                    Text("friends_profile_name")
                }
                Section {
                    TextField("friends_profile_handle", text: $handle)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("friends.profile.handle")
                    if error == "HANDLE_TAKEN" {
                        Text(LocalizedStringKey(FriendsLogic.errorKey(error))).font(.footnote).foregroundStyle(.red)
                            .accessibilityIdentifier("friends.profile.handle.error")
                    }
                } header: {
                    Text("friends_profile_handle")
                } footer: {
                    Text("friends_profile_handle_hint").foregroundStyle(handleValid ? Color.secondary : Color.red)
                }
                Section {
                    Toggle("friends_profile_find_handle", isOn: $findByHandle)
                        .accessibilityIdentifier("friends.profile.findhandle")
                    Toggle("friends_profile_find_email", isOn: $findByEmail)
                        .accessibilityIdentifier("friends.profile.findemail")
                } footer: {
                    Text("friends_profile_find_footer")
                }
                if !error.isEmpty && error != "HANDLE_TAKEN" {
                    Section {
                        Text(LocalizedStringKey(FriendsLogic.errorKey(error))).foregroundStyle(.red)
                            .accessibilityIdentifier("friends.profile.error")
                    }
                }
            } else if friends.profileLoading || friends.profileError.isEmpty {
                Section {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("teams_loading").foregroundStyle(.secondary)
                    }
                }
            } else {
                Section {
                    Text(LocalizedStringKey(FriendsLogic.errorKey(friends.profileError))).foregroundStyle(.red)
                    Button("common_retry") { Task { await friends.loadProfile() } }
                        .accessibilityIdentifier("friends.profile.retry")
                }
            }
        }
        .navigationTitle("friends_profile_title")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button("common_save", action: save)
                    .disabled(!canSave)
                    .accessibilityIdentifier("friends.profile.save")
            }
        }
        .hidesTabBarWhenPushed(in: .friends)
        .task(id: friends.uid) { await friends.loadProfile() }
        // The first profile fills the form; a later one (this screen's own save) re-bases it. Typing is never overwritten by a refetch.
        .onChange(of: friends.profile, initial: true) { _, profile in
            guard let profile, loaded == nil else { return }
            fill(from: profile)
        }
        .onChange(of: friends.authority) { _, _ in loaded = nil; error = "" }
        .accessibilityIdentifier("friends.profile")
    }

    private func fill(from profile: FriendProfile) {
        loaded = profile
        name = profile.displayName
        handle = profile.handle ?? ""
        findByHandle = profile.findByHandle
        findByEmail = profile.findByEmail
    }

    private func save() {
        let sent = changes
        guard canSave, !sent.isEmpty else { return }
        saving = true; error = ""
        Task {
            if let code = await friends.saveProfile(sent) {
                error = code
            } else if let profile = friends.profile {
                fill(from: profile)
            }
            saving = false
        }
    }
}
