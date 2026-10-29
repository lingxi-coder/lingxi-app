import SwiftUI
import UIKit

struct TeamsPage: View {
    let initialInvite: String?
    let onOpenAccount: () -> Void
    @State private var model = TeamsModel()
    @State private var name = ""
    @State private var code = ""
    @State private var maxUses = 1
    @State private var ttl = 86400
    @State private var inviteRole = "member"
    @State private var confirmation: TeamConfirmation?

    var body: some View {
        List {
            Section { Text("teams_disclosure").font(.footnote).foregroundStyle(.secondary) }
            if let error = model.errorKey { Section { Text(LocalizedStringKey(error)).foregroundStyle(.red).accessibilityIdentifier("teams.error") } }
            if model.uid == nil {
                Section { Button("teams_signin", action: onOpenAccount) }
            } else {
                if model.loading { ProgressView().accessibilityLabel(Text("teams_loading")) }
                Section("teams_my") {
                    ForEach(model.teams) { team in
                        Button { model.select(team.id); inviteRole = "member" } label: {
                            HStack { VStack(alignment: .leading) { Text(team.name); Text(roleKey(team.role)).font(.caption).foregroundStyle(.secondary) }; Spacer(); Text(team.memberCount, format: .number) }
                        }
                        .accessibilityIdentifier("teams.team.\(team.id)")
                    }
                }
                creationSection
                joinSection
                if let detail = model.detail { membersSection(detail); if detail.team.role != "member" { invitesSection(detail) } }
            }
        }
        .navigationTitle(Text("teams_title"))
        .toolbar { Button("teams_refresh", systemImage: "arrow.clockwise") { model.scheduleRefresh() }.disabled(model.busy) }
        .refreshable { await model.refresh() }
        .disabled(model.busy)
        .task { model.start(); if let initialInvite { code = initialInvite } }
        .onChange(of: initialInvite) { _, value in if let value { code = value } }
        .onChange(of: model.uid) { _, _ in confirmation = nil; name = "" }
        .onDisappear { model.stop() }
        .confirmationDialog(confirmation.map { Text(LocalizedStringKey($0.title)) } ?? Text("teams_confirm_change"), isPresented: Binding(get: { confirmation != nil }, set: { if !$0 { confirmation = nil } }), titleVisibility: .visible) {
            if let confirmation { Button("teams_confirm", role: .destructive) { execute(confirmation.input); self.confirmation = nil } }
            Button("teams_cancel", role: .cancel) { confirmation = nil }
        }
    }
    private var creationSection: some View {
        Section("teams_new") {
            TextField("teams_name", text: $name).accessibilityIdentifier("teams.name")
            Button("teams_create") { execute(["action": "create", "teamId": UUID().uuidString, "name": name.trimmingCharacters(in: .whitespacesAndNewlines)]) }
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || name.count > 80)
        }
    }
    private var joinSection: some View {
        Section {
            TextField("teams_invite_input", text: $code).textInputAutocapitalization(.never).autocorrectionDisabled().privacySensitive().accessibilityIdentifier("teams.invite")
            Button("teams_join_confirm") {
                guard let parsed = TeamInvitation.code(code) else { return }
                Task { if await model.execute(["action": "join", "code": parsed, "requestId": UUID().uuidString]) { code = "" } }
            }.disabled(TeamInvitation.code(code) == nil)
        } header: { Text("teams_join") } footer: { Text("teams_join_disclosure") }
    }
    private func membersSection(_ detail: TeamDetail) -> some View {
        Section {
            ForEach(detail.members) { member in
                HStack {
                    VStack(alignment: .leading) { Text(member.displayName); Text(roleKey(member.role)).font(.caption).foregroundStyle(.secondary) }
                    Spacer()
                    if member.uid != model.uid, member.role != "owner", detail.team.role == "owner" || (detail.team.role == "admin" && member.role == "member") {
                        Menu {
                            if detail.team.role == "owner" {
                                Button(LocalizedStringKey(member.role == "admin" ? "teams_demote" : "teams_promote")) {
                                    confirm("teams_confirm_change", ["action": "role", "teamId": detail.team.id, "uid": member.uid, "generation": member.generation, "role": member.role == "admin" ? "member" : "admin"])
                                }
                            }
                            Button("teams_remove", role: .destructive) { confirm("teams_confirm_change", ["action": "remove", "teamId": detail.team.id, "uid": member.uid, "generation": member.generation]) }
                        } label: { Image(systemName: "ellipsis.circle").accessibilityLabel(Text("teams_members")) }
                    }
                }
            }
            Button(LocalizedStringKey(detail.team.role == "owner" ? "teams_delete" : "teams_leave"), role: .destructive) {
                var input: [String: Any] = ["action": detail.team.role == "owner" ? "delete" : "leave", "teamId": detail.team.id]
                if detail.team.role != "owner" { input["generation"] = detail.team.generation }
                confirm(detail.team.role == "owner" ? "teams_confirm_delete" : "teams_confirm_leave", input)
            }
        } header: { Text(detail.team.name + " · " + String(localized: "teams_members")) }
    }
    private func invitesSection(_ detail: TeamDetail) -> some View {
        Section("teams_invites") {
            if detail.team.role == "owner" { Picker("teams_members", selection: $inviteRole) { Text("teams_member").tag("member"); Text("teams_admin").tag("admin") } }
            Stepper(value: $maxUses, in: 1...50) { HStack { Text("teams_uses"); Spacer(); Text(maxUses, format: .number) } }
            Picker("teams_expiry", selection: $ttl) { Text("teams_hour").tag(3600); Text("teams_day").tag(86400); Text("teams_week").tag(604800) }
            Button("teams_invite_create") { execute(["action": "invite", "teamId": detail.team.id, "requestId": UUID().uuidString, "role": detail.team.role == "owner" ? inviteRole : "member", "maxUses": maxUses, "ttlSeconds": ttl]) }
            if !model.invitationLink.isEmpty { Button { UIPasteboard.general.string = model.invitationLink } label: { Label("teams_copy", systemImage: "doc.on.doc") }; Text(model.invitationLink).font(.caption).textSelection(.enabled).privacySensitive() }
            ForEach(detail.invites) { invite in
                HStack {
                    VStack(alignment: .leading) { Text(roleKey(invite.role)); Text("\(invite.uses)/\(invite.maxUses) · \(invite.expiresAt)").font(.caption).foregroundStyle(.secondary) }
                    Spacer()
                    if invite.revoked { Text("teams_revoked").font(.caption) }
                    else { Button("teams_revoke") { execute(["action": "revokeInvite", "teamId": detail.team.id, "inviteId": invite.id]) }.disabled(detail.team.role != "owner" && invite.role == "admin") }
                }
            }
        }
    }
    private func roleKey(_ role: String) -> LocalizedStringKey { LocalizedStringKey("teams_" + role) }
    private func execute(_ input: [String: Any]) { Task { _ = await model.execute(input) } }
    private func confirm(_ title: String, _ input: [String: Any]) { confirmation = TeamConfirmation(title: title, input: input) }
    private struct TeamConfirmation { let title: String; let input: [String: Any] }
}
