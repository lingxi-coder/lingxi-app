import AppIntents
import Foundation
import UniformTypeIdentifiers

private struct LocalAppAgentNativeStart: Decodable, Sendable {
    let runId: String
}

private struct LocalAppAgentIntentReceipt: Encodable, Sendable {
    let appId: String
    let revision: UInt64
    let runId: String
    let text: String
    let output: LocalAppJSON?
}

enum LocalAppAgentIntentInput {
    static let maximumFiles = 8
    static let maximumFileBytes = 25 * 1024 * 1024
    static let maximumTotalFileBytes = 50 * 1024 * 1024
    static let maximumSchemaBytes = 256 * 1024

    static func attachments(_ files: [IntentFile]) throws -> [LocalAppJSON] {
        guard files.count <= maximumFiles else {
            throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "The Agent accepts at most 8 files.")
        }
        var totalBytes = 0
        return try files.map { file in
            let name = file.filename
            guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  name.utf8.count <= 255,
                  !name.contains("/"), !name.contains("\\"),
                  !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
                throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "A file has an invalid name.")
            }
            let mediaType = file.type?.preferredMIMEType ?? "application/octet-stream"
            guard !mediaType.isEmpty, mediaType.utf8.count <= 127,
                  !mediaType.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
                throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "A file has an invalid media type.")
            }
            let availableBytes = min(maximumFileBytes, maximumTotalFileBytes - totalBytes)
            let bytes = try boundedData(for: file, maximumBytes: availableBytes)
            guard bytes.count <= maximumFileBytes, totalBytes <= maximumTotalFileBytes - bytes.count else {
                throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "Files must be at most 25 MiB each and 50 MiB total.")
            }
            totalBytes += bytes.count
            return .object([
                "name": .string(name),
                "mediaType": .string(mediaType),
                "base64": .string(bytes.base64EncodedString())
            ])
        }
    }

    static func outputSchema(_ text: String?) throws -> LocalAppJSON? {
        guard let text, !text.isEmpty else { return nil }
        guard text.utf8.count <= maximumSchemaBytes else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Output schema must be a JSON object no larger than 256 KiB.")
        }
        do {
            let value = try JSONDecoder().decode(LocalAppJSON.self, from: Data(text.utf8))
            guard case .object = value else {
                throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Output schema must be a JSON object no larger than 256 KiB.")
            }
            return value
        } catch {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Output schema must be a JSON object no larger than 256 KiB.")
        }
    }

    private static func boundedData(for file: IntentFile, maximumBytes: Int) throws -> Data {
        guard let url = file.fileURL else {
            let data = file.data
            guard data.count <= maximumBytes else {
                throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "Files must be at most 25 MiB each and 50 MiB total.")
            }
            return data
        }
        let accessed = url.startAccessingSecurityScopedResource()
        defer { if accessed { url.stopAccessingSecurityScopedResource() } }
        let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
        guard let fileSize = attributes[.size] as? NSNumber,
              fileSize.uint64Value <= UInt64(maximumBytes) else {
            throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "Files must be at most 25 MiB each and 50 MiB total.")
        }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var data = Data()
        while data.count <= maximumBytes {
            let remaining = maximumBytes + 1 - data.count
            let chunk = try handle.read(upToCount: min(64 * 1024, remaining)) ?? Data()
            if chunk.isEmpty { break }
            data.append(chunk)
        }
        guard data.count <= maximumBytes else {
            throw LocalAppServiceFailure("INVALID_AGENT_ATTACHMENTS", "Files must be at most 25 MiB each and 50 MiB total.")
        }
        return data
    }
}

/// Shares the native Local App broker with browser runs. The intent carries the
/// complete bounded input and never waits for an external browser handoff.
@available(iOS 27.0, *)
struct RunLocalAppAgentIntent: AppIntent, LongRunningIntent, CancellableIntent {
    static let title: LocalizedStringResource = "Run Local App Agent"
    static let description = IntentDescription("Run a selected Local App Agent request with files, then return its result and run reference.")
    static var supportedModes: IntentModes { .background }
    static var authenticationPolicy: IntentAuthenticationPolicy { .requiresAuthentication }
    static var parameterSummary: some ParameterSummary {
        Summary("Run Local App Agent for app \(\.$appID)") {
            \.$revision
            \.$prompt
            \.$attachments
            \.$outputSchema
            \.$tools
            \.$skills
            \.$maxTurns
            \.$timeLimitMinutes
        }
    }

    @Parameter(title: "App ID") var appID: String
    @Parameter(title: "Release revision") var revision: Int
    @Parameter(title: "Prompt") var prompt: String
    @Parameter(title: "Files", default: []) var attachments: [IntentFile]
    @Parameter(title: "Output schema JSON") var outputSchema: String?
    @Parameter(title: "Requested tools", default: []) var tools: [String]
    @Parameter(title: "Requested Skills", default: []) var skills: [String]
    @Parameter(title: "Maximum model turns", default: 20) var maxTurns: Int
    @Parameter(title: "Time limit in minutes", default: 10) var timeLimitMinutes: Int

    func perform() async throws -> some ReturnsValue<String> & ProvidesDialog {
        guard revision > 0, let revisionValue = UInt64(exactly: revision),
              maxTurns > 0, maxTurns <= 100, timeLimitMinutes > 0, timeLimitMinutes <= 60 else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "Use a positive release revision, at most 100 turns, and a time limit up to 60 minutes.")
        }
        guard !appID.isEmpty, appID.utf8.count <= 128,
              !appID.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 }) else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "The Local App ID is invalid.")
        }
        let requestID = UUID().uuidString.lowercased()
        let client = LocalApps.library.client
        let cancellation = LocalAppAgentIntentCancellation(client: client, appID: appID, revision: revisionValue)
        let taskProgress = progress

        let receipt = try await performBackgroundTask(operation: {
            let input = try Self.nativeInput(
                requestID: requestID,
                prompt: self.prompt,
                attachments: self.attachments,
                outputSchema: self.outputSchema,
                tools: self.tools,
                skills: self.skills,
                maxTurns: self.maxTurns,
                timeoutMs: self.timeLimitMinutes * 60_000
            )
            let inputJSON = try LocalAppJSON.encode(input)
            let started = try await client.agentHost([
                "op": .string("startNative"),
                "appId": .string(self.appID),
                "revision": .number(Double(revisionValue)),
                "input": inputJSON
            ], as: LocalAppAgentNativeStart.self)
            try await cancellation.didStart(started.runId)
            return try await Self.waitForCompletion(
                client: client,
                cancellation: cancellation,
                appID: self.appID,
                revision: revisionValue,
                requestID: requestID,
                runID: started.runId,
                progress: taskProgress
            )
        }, onCancel: { _ in
            Task { await cancellation.cancel() }
        })
        let data = try JSONEncoder().encode(receipt)
        let value = String(decoding: data, as: UTF8.self)
        return .result(value: value, dialog: IntentDialog("The Local App Agent task completed. Its run reference and result are ready."))
    }

    private static func nativeInput(
        requestID: String,
        prompt: String,
        attachments: [IntentFile],
        outputSchema: String?,
        tools: [String],
        skills: [String],
        maxTurns: Int,
        timeoutMs: Int
    ) throws -> [String: LocalAppJSON] {
        guard !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              prompt.utf8.count <= 256 * 1024,
              tools.count <= 128, Set(tools).count == tools.count,
              skills.count <= 128, Set(skills).count == skills.count else {
            throw LocalAppServiceFailure("INVALID_AGENT_INPUT", "The prompt or file count is outside the Agent limits.")
        }
        var fields: [String: LocalAppJSON] = [
            "requestId": .string(requestID),
            "prompt": .string(prompt),
            "attachments": .array(try LocalAppAgentIntentInput.attachments(attachments)),
            "tools": .array(tools.map(LocalAppJSON.string)),
            "skills": .array(skills.map(LocalAppJSON.string)),
            "maxTurns": .number(Double(maxTurns)),
            "timeoutMs": .number(Double(timeoutMs))
        ]
        if let outputSchema = try LocalAppAgentIntentInput.outputSchema(outputSchema) {
            fields["outputSchema"] = outputSchema
        }
        return fields
    }

    private static func waitForCompletion(
        client: LocalAppServiceClient,
        cancellation: LocalAppAgentIntentCancellation,
        appID: String,
        revision: UInt64,
        requestID: String,
        runID: String,
        progress: Progress
    ) async throws -> LocalAppAgentIntentReceipt {
        var deadline = Date().addingTimeInterval(3_600)
        while Date() < deadline {
            try Task.checkCancellation()
            if await cancellation.isCancelled { throw CancellationError() }
            let response = try await client.agentHost([
                "op": .string("tasks.list"),
                "appId": .string(appID),
                "revision": .number(Double(revision)),
                "requestId": .string(requestID)
            ], as: LocalAppAgentTaskListResponse.self)
            guard response.appId == appID, response.revision == revision,
                  let task = response.tasks.first(where: { $0.requestId == requestID && $0.runId == runID }) else {
                throw LocalAppServiceFailure("AGENT_TASK_MISSING", "The Local App host did not return the admitted task.")
            }
            if let observed = task.progress, observed.turnLimit > 0 {
                LocalAppAgentProgressReporting.update(
                    progress,
                    completedModelTurns: observed.completedModelTurns,
                    turnLimit: observed.turnLimit
                )
            } else {
                LocalAppAgentProgressReporting.update(
                    progress,
                    description: task.phase ?? "Agent task \(task.state); waiting for observed model progress"
                )
            }
            deadline = Date(timeIntervalSince1970: TimeInterval(task.createdAt) / 1_000)
                .addingTimeInterval(TimeInterval(min(task.limits.timeoutMs, 3_600_000)) / 1_000)

            if task.state == "succeeded" {
                guard let result = task.result else {
                    throw LocalAppServiceFailure("INVALID_AGENT_RESULT", "The completed Agent task has no result.")
                }
                LocalAppAgentProgressReporting.finish(progress, success: true)
                return LocalAppAgentIntentReceipt(appId: appID, revision: revision, runId: runID, text: result.text, output: result.output)
            }
            if task.isTerminal {
                LocalAppAgentProgressReporting.finish(progress, success: false)
                throw LocalAppServiceFailure(task.error?.code ?? "AGENT_TASK_FAILED", task.error?.message ?? "The Agent task ended as \(task.state).")
            }
            do { try await Task.sleep(for: .seconds(1)) } catch { throw CancellationError() }
        }
        await cancellation.cancel()
        LocalAppAgentProgressReporting.finish(progress, success: false)
        throw LocalAppServiceFailure("AGENT_TASK_TIMEOUT", "The Local App Agent task exceeded its configured time limit.")
    }
}

@available(iOS 27.0, *)
private actor LocalAppAgentIntentCancellation {
    private let client: LocalAppServiceClient
    private let appID: String
    private let revision: UInt64
    private var runID: String?
    private(set) var isCancelled = false

    init(client: LocalAppServiceClient, appID: String, revision: UInt64) {
        self.client = client
        self.appID = appID
        self.revision = revision
    }

    func didStart(_ runID: String) async throws {
        self.runID = runID
        if isCancelled {
            await cancelRun(runID)
            throw CancellationError()
        }
    }

    func cancel() async {
        isCancelled = true
        if let runID { await cancelRun(runID) }
    }

    private func cancelRun(_ runID: String) async {
        _ = try? await client.agentHost([
            "op": .string("tasks.cancel"),
            "appId": .string(appID),
            "revision": .number(Double(revision)),
            "runId": .string(runID)
        ], as: LocalAppJSON.self)
    }
}
