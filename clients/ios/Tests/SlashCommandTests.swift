import XCTest

@testable import LingxiCode

#if canImport(engine_mobileFFI)
    import engine_mobileFFI
#endif

#if canImport(engine_mobileFFI)

    @MainActor
    final class SlashCommandTests: XCTestCase {
        private func makeSource() -> EngineConversationSource {
            let config = EngineConfig(
                apiBase: "https://api.anthropic.com",
                apiKey: "",
                model: "",
                appSandboxRoot: NSTemporaryDirectory(),
                projectCwd: nil
            )
            let source = EngineConversationSource(config: config)
            source.setCommandSubmitterForTesting { _ in }
            return source
        }

        private func command(
            name: String,
            description: String,
            aliases: [String] = [],
            argumentHint: String? = nil,
            menuDescription: String? = nil,
            hidden: Bool = false
        ) -> SlashCommandDto {
            SlashCommandDto(
                name: name,
                description: description,
                source: "builtin",
                aliases: aliases,
                argumentHint: argumentHint,
                menuDescription: menuDescription,
                hidden: hidden
            )
        }

        private func flushTasks(_ count: Int = 6) async {
            for _ in 0..<count { await Task.yield() }
        }

        func testMatcherSortsAliasesAndKeepsHiddenToExactMatches() {
            let catalog = [
                ConversationSlashCommand(
                    name: "help",
                    description: "Show help",
                    aliases: ["h"],
                    argumentHint: "[topic]",
                    menuDescription: nil,
                    source: "builtin",
                    hidden: false
                ),
                ConversationSlashCommand(
                    name: "hello",
                    description: "Greet",
                    aliases: [],
                    argumentHint: nil,
                    menuDescription: nil,
                    source: "builtin",
                    hidden: false
                ),
                ConversationSlashCommand(
                    name: "heapdump",
                    description: "Hidden command",
                    aliases: ["hd"],
                    argumentHint: nil,
                    menuDescription: nil,
                    source: "builtin",
                    hidden: true
                ),
            ]

            XCTAssertEqual(
                SlashCommandMatcher.suggestions(
                    for: "/h", catalog: catalog
                ).map(\.command.name),
                ["help", "hello"]
            )
            XCTAssertEqual(
                SlashCommandMatcher.suggestions(
                    for: "/heapdump", catalog: catalog
                ).map(\.command.name),
                ["heapdump"]
            )
            XCTAssertTrue(
                SlashCommandMatcher.suggestions(
                    for: "/", catalog: catalog
                )
                    .map(\.command.name)
                    .allSatisfy { $0 != "heapdump" }
            )
            XCTAssertTrue(
                SlashCommandMatcher.suggestions(
                    for: "/help topic", catalog: catalog
                ).isEmpty
            )
            XCTAssertEqual(
                SlashCommandMatcher.argumentHint(for: "/help topic", catalog: catalog),
                "[topic]"
            )
        }

        func testCatalogReducerUpdatesSlashCommandsAndSkills() {
            let source = makeSource()

            source.applyForTesting(.slashCommandCatalog(commands: [
                command(
                    name: "help",
                    description: "Show help",
                    aliases: ["h"],
                    argumentHint: "[topic]",
                    menuDescription: "Help",
                    hidden: false
                ),
            ]))

            XCTAssertTrue(source.model.slashCommandsLoaded)
            XCTAssertEqual(source.model.slashCommands.map(\.name), ["help"])
            XCTAssertEqual(source.model.slashCommands.first?.aliases, ["h"])
            XCTAssertEqual(source.model.slashCommands.first?.argumentHint, "[topic]")
            XCTAssertEqual(source.model.skills.first?.triggers, ["/help", "/h"])

            source.applyForTesting(.commandsChanged(commands: [
                command(
                    name: "review",
                    description: "Review diff",
                    aliases: ["rv"],
                    argumentHint: nil,
                    menuDescription: "Review",
                    hidden: false
                ),
            ]))

            XCTAssertEqual(source.model.slashCommands.map(\.name), ["review"])
            XCTAssertEqual(source.model.skills.map(\.name), ["review"])
        }

        func testSendRoutesExactSlashToRunSlashAndUnknownSlashToPrompt() async throws {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.slashCommandCatalog(commands: [
                command(name: "help", description: "Show help", aliases: ["h"])
            ]))

            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                submitted.append(command)
            }

            XCTAssertNotNil(source.send("/help topic"))
            await flushTasks()
            guard case let .runSlashCommand(raw, turnId) = try XCTUnwrap(submitted.first) else {
                return XCTFail("expected runSlashCommand")
            }
            XCTAssertEqual(raw, "/help topic")
            XCTAssertNotNil(turnId)

            submitted.removeAll()
            source.applyForTesting(.slashCommandResult(turnId: turnId, display: "help output", isError: false))
            XCTAssertNotNil(source.send("/unknown"))
            await flushTasks()
            guard case let .sendPrompt(text, _, _, unknownTurnId) = try XCTUnwrap(submitted.first) else {
                return XCTFail("expected sendPrompt for unknown slash")
            }
            XCTAssertEqual(text, "/unknown")
            XCTAssertNotNil(unknownTurnId)
        }

        func testLocalSlashCommandResultAppendsCommandOutputAndClearsPending() async throws {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.slashCommandCatalog(commands: [
                command(name: "help", description: "Show help")
            ]))

            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { command in
                submitted.append(command)
            }

            let token = try XCTUnwrap(source.send("/help"))
            await flushTasks()
            XCTAssertTrue(source.model.slashCommandPending)

            source.applyForTesting(.slashCommandResult(
                turnId: token.clientTurnId,
                display: "Available commands",
                isError: false
            ))

            XCTAssertFalse(source.model.slashCommandPending)
            XCTAssertFalse(source.model.streaming)
            XCTAssertEqual(source.model.turnCompletion?.token, token)
            XCTAssertEqual(source.model.turnCompletion?.outcome, .completed)
            XCTAssertEqual(source.model.turnCompletion?.finalAssistantText, "")
            guard case let .commandOutput(output)? = source.model.items.last else {
                return XCTFail("expected command output card")
            }
            XCTAssertEqual(output.command, "/help")
            XCTAssertEqual(output.text, "Available commands")
            XCTAssertFalse(output.isError)
            XCTAssertEqual(submitted.count, 1)
        }

        func testPromptSlashCommandTransitionsIntoNormalStreamingTurn() async throws {
            let source = makeSource()
            source.model.activeSessionId = "session-a"
            source.applyForTesting(.slashCommandCatalog(commands: [
                command(name: "review", description: "Review", aliases: ["rv"])
            ]))

            var submitted: [ClientCommand] = []
            source.setCommandSubmitterForTesting { submitted.append($0) }

            let token = try XCTUnwrap(source.send("/rv staged"))
            await flushTasks()
            guard case let .runSlashCommand(raw, turnId) = try XCTUnwrap(submitted.first) else {
                return XCTFail("expected runSlashCommand")
            }
            XCTAssertEqual(raw, "/rv staged")
            XCTAssertEqual(turnId, token.clientTurnId)
            XCTAssertTrue(source.model.slashCommandPending)
            XCTAssertFalse(source.model.streaming)

            source.applyForTesting(.turnStarted(turnId: token.clientTurnId))
            XCTAssertFalse(source.model.slashCommandPending)
            XCTAssertTrue(source.model.streaming)
            source.applyForTesting(.textDelta(text: "review result"))
            source.applyForTesting(.turnEnded(
                outcome: .endTurn,
                stopReason: "end_turn",
                cost: CostDto(
                    totalUsd: 0,
                    inputTokens: 0,
                    outputTokens: 0,
                    apiCalls: 0,
                    sessionDurationSecs: 0,
                    formatted: "$0.00"
                )
            ))

            XCTAssertEqual(source.model.messages.last?.text, "review result")
            XCTAssertEqual(source.model.turnCompletion?.outcome, .completed)
        }
    }

#endif
