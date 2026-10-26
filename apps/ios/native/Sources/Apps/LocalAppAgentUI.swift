import Foundation
import SwiftUI

struct LocalAppAgentModelRef: Codable, Equatable, Sendable {
    let providerId: String
    let modelId: String
    var key: String { providerId + "/" + modelId }
}
private struct AgentModelChoice: Decodable, Identifiable, Sendable {
    let providerId: String
    let modelId: String
    let label: String
    let canSelect: Bool
    let supportsImages: Bool
    let supportsStructuredOutput: Bool
    var id: String { providerId + "/" + modelId }
}
private struct AgentCatalog: Decodable, Sendable {
    struct Server: Decodable, Identifiable, Sendable { let id: String; let name: String; let available: Bool }
    let servers: [Server]
    let models: [AgentModelChoice]
    let tools: [String: LocalAppJSON]
    let labels: [String: String]
}
private struct AgentSkillRequirement: Equatable, Identifiable, Sendable {
    let id: String
    let path: String
}
private struct AgentSettings: Codable, Sendable {
    let model: LocalAppAgentModelRef
    let tools: [String: LocalAppJSON]
    let skills: [String]
    let background: Bool
    let maxTurns: Int
    let timeoutMs: Int

    private enum CodingKeys: String, CodingKey { case model, tools, skills, background, maxTurns, timeoutMs }
    init(model: LocalAppAgentModelRef, tools: [String: LocalAppJSON], skills: [String], background: Bool, maxTurns: Int, timeoutMs: Int) {
        self.model = model
        self.tools = tools
        self.skills = skills
        self.background = background
        self.maxTurns = maxTurns
        self.timeoutMs = timeoutMs
    }
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        model = try values.decode(LocalAppAgentModelRef.self, forKey: .model)
        tools = try values.decode([String: LocalAppJSON].self, forKey: .tools)
        skills = try values.decodeIfPresent([String].self, forKey: .skills) ?? []
        background = try values.decode(Bool.self, forKey: .background)
        maxTurns = try values.decode(Int.self, forKey: .maxTurns)
        timeoutMs = try values.decode(Int.self, forKey: .timeoutMs)
    }
}

@MainActor
struct LocalAppAgentSettingsView: View {
    let client: LocalAppServiceClient
    let appID: String
    let release: UnifiedAppRelease
    let onConfigurationChanged: @MainActor () async -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var catalog: AgentCatalog?
    @State private var model = ""
    @State private var tools = Set<String>()
    @State private var skills = Set<String>()
    @State private var turns = 20
    @State private var minutes = 10
    @State private var busy = false
    @State private var error: String?

    private var declared: [String] {
        guard let requirements = release.manifest.agent, case .object(let agent) = requirements, let toolValues = agent["tools"], case .array(let values) = toolValues else { return [] }
        return values.compactMap(\.text)
    }
    private var declaredSkills: [AgentSkillRequirement] {
        guard let requirements = release.manifest.agent, case .object(let agent) = requirements,
              let skillValues = agent["skills"], case .array(let values) = skillValues else { return [] }
        return values.compactMap { value in
            guard case .object(let fields) = value, let id = fields["id"]?.text,
                  let path = fields["path"]?.text else { return nil }
            return AgentSkillRequirement(id: id, path: path)
        }.sorted { $0.id < $1.id }
    }
    var body: some View {
        NavigationStack {
            Form {
                if let catalog {
                    Section("Agent 模型") {
                        Picker("模型", selection: $model) {
                            Text("请选择模型").tag("")
                            ForEach(catalog.models.filter(\.canSelect)) { choice in
                                Text("\(choice.label) · \(choice.providerId)").tag(choice.id)
                            }
                        }
                        if !catalog.models.contains(where: { $0.canSelect }) {
                            Text("尚无可用模型。请先在灵犀设置中配置模型和账号。").foregroundStyle(.secondary)
                        }
                        Text("任务独立运行，只读取本次输入；使用你在灵犀配置的模型账号。").font(.footnote)
                    }
                    if !catalog.servers.isEmpty {
                        Section("已有 MCP 连接") {
                            ForEach(catalog.servers) { server in
                                if server.available { Button("读取 \(server.name) 的工具目录") { connect(server.id) }.disabled(busy) }
                                else { Text("\(server.name)：此连接需要桌面端").foregroundStyle(.secondary) }
                            }
                            Text("连接后仍需逐项授权本应用请求的工具。").font(.footnote)
                        }
                    }
                    Section("允许此应用使用的工具") {
                        if declared.isEmpty { Text("此版本不请求业务工具。").foregroundStyle(.secondary) }
                        ForEach(declared.filter { catalog.tools[$0] != nil }, id: \.self) { id in
                            Toggle(catalog.labels[id] ?? id, isOn: Binding(get: { tools.contains(id) }, set: { value in
                                if value { tools.insert(id) } else { tools.remove(id) }
                            }))
                        }
                        Text("允许工具后，敏感操作仍由灵犀请求批准。工具或连接配置变化后需要重新授权。").font(.footnote)
                    }
                    if !declaredSkills.isEmpty {
                        Section("允许此应用使用的 Skills") {
                            ForEach(declaredSkills) { skill in
                                Toggle(skill.id, isOn: Binding(get: { skills.contains(skill.id) }, set: { value in
                                    if value { skills.insert(skill.id) } else { skills.remove(skill.id) }
                                }))
                                Text(skill.path).font(.caption.monospaced()).foregroundStyle(.secondary)
                            }
                            Text("Skill 指令来自此不可变发布版本；Skill 不会额外授予工具权限。").font(.footnote)
                        }
                    }
                    Section("每次任务限额") {
                        Stepper("最多 \(turns) 轮", value: $turns, in: 1...100)
                        Stepper("最多 \(minutes) 分钟", value: $minutes, in: 1...60)
                        Text("停止应用或撤销授权会停止任务。浏览器页面关闭不等于停止；模型费用按实际用量计量。").font(.footnote)
                    }
                    Section {
                        Button("保存授权", action: save).disabled(model.isEmpty || busy)
                        Button("撤销 Agent 授权", role: .destructive, action: revoke).disabled(busy)
                    }
                } else { ProgressView("读取宿主模型与工具…") }
                if let error { Section { Text(error).foregroundStyle(.red) } }
            }
            .navigationTitle("Agent 授权")
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("关闭") { dismiss() } } }
            .task { await load() }
        }
    }
    private func call<T: Decodable>(_ fields: [String: LocalAppJSON], as: T.Type) async throws -> T {
        let value = try await client.agentHost(String(decoding: JSONEncoder().encode(fields), as: UTF8.self))
        return try JSONDecoder().decode(T.self, from: JSONEncoder().encode(value))
    }
    private func load() async {
        do {
            catalog = try await call(["op": .string("catalog")], as: AgentCatalog.self)
            if let saved = try await call(["op": .string("settings"), "appId": .string(appID)], as: AgentSettings?.self) {
                model = saved.model.key; tools = Set(saved.tools.keys); skills = Set(saved.skills)
                turns = saved.maxTurns; minutes = max(1, saved.timeoutMs / 60_000)
            }
        } catch { self.error = error.localizedDescription }
    }
    private func connect(_ serverID: String) {
        Task { busy = true; defer { busy = false }; do {
            catalog = try await call(["op": .string("connectMcp"), "serverId": .string(serverID)], as: AgentCatalog.self)
            await onConfigurationChanged()
        } catch { self.error = error.localizedDescription } }
    }
    private func save() {
        guard let catalog, let selected = catalog.models.first(where: { $0.id == model && $0.canSelect }) else { return }
        let allowedSkills = Set(declaredSkills.map(\.id))
        let settings = AgentSettings(model: .init(providerId: selected.providerId, modelId: selected.modelId),
            tools: catalog.tools.filter { tools.contains($0.key) && declared.contains($0.key) },
            skills: skills.filter { allowedSkills.contains($0) }.sorted(), background: false,
            maxTurns: turns, timeoutMs: minutes * 60_000)
        Task { busy = true; defer { busy = false }; do {
            let _: LocalAppJSON = try await call(["op": .string("configure"), "appId": .string(appID), "settings": try .encode(settings)], as: LocalAppJSON.self)
            await onConfigurationChanged()
            dismiss()
        } catch { self.error = error.localizedDescription } }
    }
    private func revoke() {
        Task { busy = true; defer { busy = false }; do {
            let _: LocalAppJSON = try await call(["op": .string("configure"), "appId": .string(appID), "settings": .null], as: LocalAppJSON.self)
            await onConfigurationChanged()
            dismiss()
        } catch { self.error = error.localizedDescription } }
    }
}

private struct AgentInteraction: Decodable, Identifiable, Sendable {
    struct Question: Decodable, Sendable {
        struct Option: Decodable, Sendable { let label: String; let description: String }
        let question: String
        let multiSelect: Bool
        let options: [Option]
    }
    let requestId: String
    let runId: String
    let appId: String
    let type: String
    let toolName: String?
    let input: LocalAppJSON?
    let questions: [Question]?
    var id: String { requestId }
}

/// Polls the trusted native endpoint. App JavaScript never receives this data.
@MainActor
struct LocalAppAgentInteractions: View {
    let client: LocalAppServiceClient
    let appID: String
    @State private var pending: AgentInteraction?
    var body: some View {
        Color.clear.frame(width: 0, height: 0)
            .task(id: appID) {
                while !Task.isCancelled {
                    do {
                        let request = "{\"op\":\"interactions\"}"
                        let value = try await client.agentHost(request)
                        let interactions = try JSONDecoder().decode([AgentInteraction].self, from: JSONEncoder().encode(value))
                        let current = interactions.first { $0.appId == appID }
                        if current?.id != pending?.id { pending = current }
                    } catch is CancellationError { return }
                    catch { /* The application exposes unavailable-host errors through get/run. */ }
                    do { try await Task.sleep(for: .milliseconds(750)) } catch { return }
                }
            }
            .sheet(item: $pending) { interaction in
                LocalAppAgentInteractionSheet(client: client, interaction: interaction)
                    .interactiveDismissDisabled()
            }
    }
}

@MainActor
private struct LocalAppAgentInteractionSheet: View {
    let client: LocalAppServiceClient
    let interaction: AgentInteraction
    @Environment(\.dismiss) private var dismiss
    @State private var selected: [String: Set<String>] = [:]
    @State private var custom: [String: String] = [:]
    @State private var busy = false
    @State private var error: String?
    private var permission: Bool { interaction.type == "permission" }
    private var input: String {
        guard let value = interaction.input, let data = try? JSONEncoder().encode(value),
              let object = try? JSONSerialization.jsonObject(with: data),
              let formatted = try? JSONSerialization.data(withJSONObject: object, options: [.prettyPrinted, .sortedKeys]) else { return "" }
        return String(decoding: formatted, as: UTF8.self)
    }
    var body: some View {
        NavigationStack {
            Form {
                Section("请求来源") { Text(interaction.appId).font(.caption).textSelection(.enabled) }
                if permission {
                    Section("工具请求：\(interaction.toolName ?? "Agent")") {
                        Text(input).font(.system(.footnote, design: .monospaced)).textSelection(.enabled)
                    }
                } else {
                    ForEach(interaction.questions ?? [], id: \.question) { question in
                        Section(question.question) {
                            ForEach(question.options, id: \.label) { option in
                                Button {
                                    var choices = selected[question.question, default: []]
                                    if question.multiSelect { if choices.contains(option.label) { choices.remove(option.label) } else { choices.insert(option.label) } }
                                    else { choices = [option.label] }
                                    selected[question.question] = choices
                                } label: {
                                    Label { VStack(alignment: .leading) { Text(option.label); Text(option.description).font(.footnote).foregroundStyle(.secondary) } }
                                        icon: { Image(systemName: selected[question.question, default: []].contains(option.label) ? "checkmark.circle.fill" : "circle") }
                                }
                            }
                            TextField("其他回答", text: Binding(get: { custom[question.question] ?? "" }, set: { custom[question.question] = $0 }))
                        }
                    }
                }
                if let error { Text(error).foregroundStyle(.red) }
                Section {
                    Button(permission ? "仅允许此次操作" : "提交回答") { respond(approved: true) }.disabled(busy || (!permission && !complete))
                    Button(permission ? "拒绝" : "取消任务", role: .destructive) { respond(approved: false) }.disabled(busy)
                }
            }
            .navigationTitle(permission ? "灵犀工具审批" : "Agent 需要回答")
        }
    }
    private var complete: Bool { (interaction.questions ?? []).allSatisfy { !(custom[$0.question] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !(selected[$0.question] ?? []).isEmpty } }
    private func respond(approved: Bool) {
        Task {
            busy = true; defer { busy = false }
            do {
                var fields: [String: LocalAppJSON] = ["op": .string("answer"), "runId": .string(interaction.runId), "requestId": .string(interaction.requestId)]
                if permission { fields["approved"] = .bool(approved) }
                else if !approved { fields["op"] = .string("cancel") }
                else {
                    let answers = (interaction.questions ?? []).reduce(into: [String: LocalAppJSON]()) { result, question in
                        let customValue = (custom[question.question] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                        let answer = customValue.isEmpty ? question.options.filter { selected[question.question, default: []].contains($0.label) }.map(\.label).joined(separator: ", ") : customValue
                        result[question.question] = .string(answer)
                    }
                    fields["answers"] = .object(answers)
                }
                _ = try await client.agentHost(String(decoding: JSONEncoder().encode(fields), as: UTF8.self))
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

/// Active Local App tasks are shown from host-observed state; the view never
/// invents percentage progress or maps cancellation to another app's task.
@MainActor
struct LocalAppAgentTasks: View {
    let client: LocalAppServiceClient
    let appID: String
    let revision: UInt64
    @State private var tasks: [LocalAppAgentTaskSummary] = []
    @State private var error: String?
    @State private var cancelling = Set<String>()

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if tasks.isEmpty {
                Text("No Agent tasks for this app version.").font(.footnote).foregroundStyle(.secondary)
            }
            ForEach(tasks) { task in
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(String(localized: "session_details_status")): \(task.state)")
                        .font(.subheadline.weight(.medium))
                        .fixedSize(horizontal: false, vertical: true)
                    if let phase = task.phase {
                        Text(phase)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Text("\(String(localized: "cron_run_id_label")): \(task.requestId)")
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if let progress = task.progress, progress.turnLimit > 0 {
                        Text("Completed \(progress.completedModelTurns) model turns; limit \(progress.turnLimit)")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let taskError = task.error {
                        Text(String(localized: "cron_error_section"))
                            .font(.subheadline.weight(.medium))
                        Text(taskError.code)
                            .font(.caption.monospaced())
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                        Text(taskError.message)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if !task.isTerminal {
                        Button("Stop", role: .destructive) { cancel(task) }
                            .disabled(cancelling.contains(task.runId))
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 8)
            }
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: "\(appID):\(revision)") {
            while !Task.isCancelled {
                do {
                    let response = try await client.agentHost([
                        "op": .string("tasks.list"),
                        "appId": .string(appID),
                        "revision": .number(Double(revision))
                    ], as: LocalAppAgentTaskListResponse.self)
                    guard response.appId == appID, response.revision == revision else {
                        throw LocalAppServiceFailure("INVALID_AGENT_TASK", "The host returned tasks for another app version.")
                    }
                    tasks = response.tasks.sorted { $0.sequence > $1.sequence }
                    error = nil
                } catch is CancellationError { return }
                catch { self.error = error.localizedDescription }
                do { try await Task.sleep(for: .seconds(1)) } catch { return }
            }
        }
    }

    private func cancel(_ task: LocalAppAgentTaskSummary) {
        Task {
            cancelling.insert(task.runId)
            defer { cancelling.remove(task.runId) }
            do {
                let _: LocalAppJSON = try await client.agentHost([
                    "op": .string("tasks.cancel"),
                    "appId": .string(appID),
                    "revision": .number(Double(revision)),
                    "runId": .string(task.runId)
                ], as: LocalAppJSON.self)
            } catch { self.error = error.localizedDescription }
        }
    }
}
