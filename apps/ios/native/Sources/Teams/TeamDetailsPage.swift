import SwiftUI
import UIKit

/// The selected team's details (spec §6.4; Desktop `TeamDetailsDrawer`): its members, invitations and Bots, and
/// leaving or deleting it. Pushed from the team switcher on phones and a side pane on wide layouts.
struct TeamDetailsPage: View {
    @Environment(TeamsWorkspace.self) private var workspace
    @Environment(\.locale) private var locale
    /// Opens a conversation (a Bot's DM) in the Teams tab.
    let onOpenConversation: (String) -> Void
    /// The team was left, deleted or lost: there is nothing left to show.
    let onClose: () -> Void
    /// A Close button in the navigation bar (the side pane has no back button).
    var showsClose = false

    private enum Segment: String, Identifiable {
        case members, invites, bots
        var id: String { rawValue }
        var title: LocalizedStringKey {
            switch self {
            case .members: "teams_members"
            case .invites: "teams_invites"
            case .bots: "teams_bots_title"
            }
        }
    }

    private struct Confirmation: Identifiable {
        enum Kind { case role, remove, leave, delete }
        let id = UUID()
        let kind: Kind
        /// Who or what it is about, shown under the question.
        let subject: String
        let input: [String: Any]
    }

    @State private var segment = Segment.members
    @State private var maxUses = 1
    @State private var ttl = 86400
    @State private var inviteRole = "member"
    @State private var confirmation: Confirmation?
    @State private var unbinding: TeamBotCard?
    @State private var copied = false

    var body: some View {
        Group {
            if let team = workspace.team {
                content(team)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .navigationTitle("teams_team_details")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if showsClose {
                ToolbarItem(placement: .cancellationAction) { Button("common_close", action: onClose) }
            }
        }
        .hidesTabBarWhenPushed(in: .teams)
        // A change of account or membership invalidates what the person was about to confirm.
        .onChange(of: workspace.authority) { _, _ in confirmation = nil; unbinding = nil }
        .onChange(of: workspace.invitationLink) { _, _ in copied = false }
        // Leaving, deleting or losing the team leaves nothing to show.
        .onChange(of: workspace.team == nil) { _, gone in if gone { onClose() } }
        .accessibilityIdentifier("teams.details")
    }

    // MARK: Content

    private func segments(for team: TeamSummary) -> [Segment] {
        TeamRoles.canInvite(role: team.role) ? [.members, .invites, .bots] : [.members, .bots]
    }

    private func content(_ team: TeamSummary) -> some View {
        let available = segments(for: team)
        let current = available.contains(segment) ? segment : .members
        let detail = workspace.detail
        let gateShowsError = current != .bots && detail == nil && !workspace.loadError.isEmpty && !workspace.detailLoading
        return List {
            header(team)
            if !workspace.error.isEmpty && !gateShowsError {
                Section {
                    Text(LocalizedStringKey(TeamsErrors.key(workspace.error))).foregroundStyle(.red).accessibilityIdentifier("teams.error")
                }
            }
            Section {
                Picker("teams_team_details", selection: Binding(get: { current }, set: { segment = $0 })) {
                    ForEach(available) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .accessibilityIdentifier("teams.details.segments")
            }
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 4, leading: 0, bottom: 4, trailing: 0))
            switch current {
            case .bots:
                botSections(team)
            case .members, .invites:
                if let detail {
                    if current == .members { memberSection(team, detail) } else { inviteSections(team, detail) }
                } else {
                    Section { loadingOrFailed(gateShowsError) }
                }
            }
            dangerSection(team)
        }
        .listStyle(.insetGrouped)
        .disabled(workspace.busy)
        .confirmationDialog(
            confirmation.map(question) ?? Text(verbatim: ""),
            isPresented: Binding(get: { confirmation != nil }, set: { if !$0 { confirmation = nil } }),
            titleVisibility: .visible, presenting: confirmation
        ) { pending in
            Button(confirmKey(pending), role: .destructive) { confirmNow(pending) }
            Button("teams_cancel", role: .cancel) {}
        } message: { pending in
            Text(verbatim: pending.subject)
        }
        .confirmationDialog(
            Text("teams_bot_unbind_title \(unbinding?.name ?? "")"),
            isPresented: Binding(get: { unbinding != nil }, set: { if !$0 { unbinding = nil } }),
            titleVisibility: .visible, presenting: unbinding
        ) { bot in
            Button("teams_bot_unbind", role: .destructive) { unbind(bot) }
                .disabled(workspace.messaging?.offline ?? true)
            Button("teams_cancel", role: .cancel) {}
        } message: { _ in
            Text("teams_bot_unbind_detail")
        }
    }

    private func header(_ team: TeamSummary) -> some View {
        Section {
            HStack(spacing: 14) {
                TeamAvatar(teamID: team.id, name: team.name, size: 48)
                VStack(alignment: .leading, spacing: 3) {
                    Text(team.name).font(.title3.weight(.semibold))
                    HStack(spacing: 4) {
                        Text(TeamsText.role(team.role))
                        Text("·")
                        Text(TeamsText.membersCount(team.memberCount))
                    }
                    .font(.subheadline).foregroundStyle(.secondary)
                }
            }
            .padding(.vertical, 2)
            .accessibilityElement(children: .combine)
            TeamsNote(text: "teams_disclosure", systemImage: "lock.shield")
        }
    }

    @ViewBuilder
    private func loadingOrFailed(_ failed: Bool) -> some View {
        if failed {
            VStack(alignment: .leading, spacing: 8) {
                Text(LocalizedStringKey(TeamsErrors.key(workspace.loadError))).foregroundStyle(.red)
                Button("common_retry") { workspace.scheduleRefresh() }
            }
            .accessibilityIdentifier("teams.details.failed")
        } else {
            HStack(spacing: 10) {
                ProgressView()
                Text("teams_loading").foregroundStyle(.secondary)
            }
        }
    }

    // MARK: Members

    private func memberSection(_ team: TeamSummary, _ detail: TeamDetail) -> some View {
        Section {
            ForEach(detail.members) { member in
                HStack(spacing: 12) {
                    PersonAvatar(id: member.uid, name: member.displayName, size: 36)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(member.displayName).font(.body.weight(.semibold)).lineLimit(1)
                        HStack(spacing: 4) {
                            if member.uid == workspace.uid { Text("teams_preview_you"); Text("·") }
                            Text(TeamsText.role(member.role))
                        }
                        .font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    if TeamRoles.canManage(viewerRole: team.role, viewerUID: workspace.uid ?? "", memberUID: member.uid, memberRole: member.role) {
                        Menu {
                            if team.role == "owner" {
                                Button(LocalizedStringKey(member.role == "admin" ? "teams_demote" : "teams_promote")) {
                                    confirmation = Confirmation(kind: .role, subject: member.displayName, input: [
                                        "action": "role", "teamId": team.id, "uid": member.uid, "generation": member.generation,
                                        "role": member.role == "admin" ? "member" : "admin",
                                    ])
                                }
                            }
                            Button("teams_remove", role: .destructive) {
                                confirmation = Confirmation(kind: .remove, subject: member.displayName, input: [
                                    "action": "remove", "teamId": team.id, "uid": member.uid, "generation": member.generation,
                                ])
                            }
                        } label: {
                            Image(systemName: "ellipsis.circle").font(.title3)
                        }
                        .accessibilityLabel(Text(verbatim: member.displayName))
                        .accessibilityIdentifier("teams.member.actions.\(member.uid)")
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("teams.member.\(member.uid)")
            }
        } header: {
            Text("teams_members")
        }
    }

    // MARK: Invites

    @ViewBuilder
    private func inviteSections(_ team: TeamSummary, _ detail: TeamDetail) -> some View {
        Section {
            if team.role == "owner" {
                Picker("teams_invite_role", selection: $inviteRole) {
                    Text("teams_member").tag("member")
                    Text("teams_admin").tag("admin")
                }
            }
            Stepper(value: $maxUses, in: 1...50) {
                HStack {
                    Text("teams_uses")
                    Spacer()
                    Text(maxUses, format: .number).foregroundStyle(.secondary)
                }
            }
            Picker("teams_expiry", selection: $ttl) {
                Text("teams_hour").tag(3600)
                Text("teams_day").tag(86400)
                Text("teams_week").tag(604800)
            }
            Button("teams_invite_create") {
                Task {
                    await workspace.execute([
                        "action": "invite", "teamId": team.id, "requestId": UUID().uuidString,
                        "role": team.role == "owner" ? inviteRole : "member", "maxUses": maxUses, "ttlSeconds": ttl,
                    ])
                }
            }
            .disabled(workspace.busy)
            .accessibilityIdentifier("teams.invite.create")
        } header: {
            Text("teams_invites")
        }
        if !workspace.invitationLink.isEmpty {
            let link = workspace.invitationLink
            Section {
                Text(link).font(.footnote.monospaced()).textSelection(.enabled).privacySensitive()
                HStack(spacing: 20) {
                    Button {
                        UIPasteboard.general.string = link
                        copied = true
                    } label: {
                        Label("teams_copy", systemImage: "doc.on.doc")
                    }
                    ShareLink(item: link) {
                        Label("teams_invite_share", systemImage: "square.and.arrow.up")
                    }
                }
                .buttonStyle(.borderless)
                if copied { Text("teams_link_copied").font(.footnote).foregroundStyle(.secondary) }
            } header: {
                Text("teams_invite_share")
            }
        }
        Section {
            ForEach(detail.invites) { invite in
                HStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(TeamsText.role(invite.role))
                        Text(inviteSummary(invite)).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    if invite.revoked {
                        Text("teams_revoked").font(.caption).foregroundStyle(.secondary)
                    } else {
                        Button("teams_revoke", role: .destructive) {
                            Task { await workspace.execute(["action": "revokeInvite", "teamId": team.id, "inviteId": invite.id]) }
                        }
                        .buttonStyle(.borderless)
                        .disabled(workspace.busy || !TeamRoles.canRevoke(viewerRole: team.role, inviteRole: invite.role))
                    }
                }
                .accessibilityElement(children: .contain)
            }
            if detail.invites.isEmpty {
                Text("teams_invite_none").font(.footnote).foregroundStyle(.secondary)
            }
        }
    }

    /// `uses/maxUses · <expiry>`; the expiry as the device formats it, or as sent when it is not a date.
    private func inviteSummary(_ invite: TeamInvite) -> String {
        let expiry: String
        if let ms = TeamsStreamLogic.parseInstant(invite.expiresAt) {
            expiry = Date(timeIntervalSince1970: Double(ms) / 1000).formatted(Date.FormatStyle(date: .abbreviated, time: .shortened).locale(locale))
        } else {
            expiry = invite.expiresAt
        }
        return "\(invite.uses)/\(invite.maxUses) · \(expiry)"
    }

    // MARK: Bots

    @ViewBuilder
    private func botSections(_ team: TeamSummary) -> some View {
        let model = workspace.messaging
        let bots = model?.bots ?? []
        let status: LocalizedStringKey? = model?.botsUnverified == true ? "teams_bots_unverified" : model?.botsLoaded == true ? nil : "teams_bots_loading"
        let offline = model?.offline ?? true
        Section {
            TeamsNote(text: "teams_bots_note", systemImage: "info.circle")
            if let status { TeamsNote(text: status, systemImage: "exclamationmark.triangle") }
        }
        Section {
            ForEach(bots) { bot in
                let existing = workspace.uid.flatMap { TeamsListLogic.findBotDm(model?.cache?.conversations ?? [], uid: $0, botKey: bot.key) }
                TeamBotCardView(
                    bot: bot, ownerName: userName(bot.ownerUid), isOwner: bot.ownerUid == workspace.uid,
                    busy: workspace.messageBusy || (offline && existing == nil),
                    onMessage: {
                        Task { if let id = await workspace.openBotDM(bot) { onOpenConversation(id) } }
                    },
                    onUnbind: { unbinding = bot })
            }
            if bots.isEmpty && status == nil {
                Text("teams_bots_none").font(.footnote).foregroundStyle(.secondary)
            }
        } header: {
            Text("teams_bots_title")
        } footer: {
            Text("teams_bots_share_hint")
        }
        if !workspace.actionError.isEmpty {
            Section {
                Text(LocalizedStringKey(TeamsErrors.key(workspace.actionError))).foregroundStyle(.red).accessibilityIdentifier("teams.error")
            }
        }
    }

    private func userName(_ id: String) -> String {
        let name = workspace.directory.first { $0.uid == id }?.displayName ?? ""
        if !name.isEmpty { return name }
        let strings = TeamsListStrings.current
        return id == workspace.uid ? strings.you : strings.member
    }

    private func unbind(_ bot: TeamBotCard) {
        Task { await workspace.run { try await workspace.messaging?.unbind(bot) } }
    }

    // MARK: Leave and delete

    private func dangerSection(_ team: TeamSummary) -> some View {
        Section {
            Button(role: .destructive) {
                if team.role == "owner" {
                    confirmation = Confirmation(kind: .delete, subject: team.name, input: ["action": "delete", "teamId": team.id])
                } else {
                    confirmation = Confirmation(kind: .leave, subject: team.name, input: ["action": "leave", "teamId": team.id, "generation": team.generation])
                }
            } label: {
                Text(LocalizedStringKey(team.role == "owner" ? "teams_delete" : "teams_leave"))
            }
            .accessibilityIdentifier("teams.details.leave")
        }
    }

    private func question(_ pending: Confirmation) -> Text {
        switch pending.kind {
        case .role, .remove: Text("teams_confirm_change")
        case .leave: Text("teams_confirm_leave")
        case .delete: Text("teams_confirm_delete")
        }
    }

    private func confirmKey(_ pending: Confirmation) -> LocalizedStringKey {
        switch pending.kind {
        case .role: LocalizedStringKey(pending.input["role"] as? String == "admin" ? "teams_promote" : "teams_demote")
        case .remove: "teams_remove"
        case .leave: "teams_leave"
        case .delete: "teams_delete"
        }
    }

    private func confirmNow(_ pending: Confirmation) {
        Task {
            let applied = await workspace.execute(pending.input)
            // The workspace moves on to another team once this one is left or deleted; the page was about the old one.
            if applied, pending.kind == .leave || pending.kind == .delete { onClose() }
        }
    }
}
