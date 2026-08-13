import SwiftUI
import UIKit
import XCTest
@testable import LingxiCode

final class AssistantMessageCollapsePolicyTests: XCTestCase {
    func testLLMActivityStateDistinguishesHiddenRunningStoppingAndPaused() {
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: false,
                streaming: false,
                isCancelling: false
            ),
            .hidden
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: true,
                isCancelling: false
            ),
            .running
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: false,
                streaming: true,
                isCancelling: false
            ),
            .running
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: true,
                isCancelling: true
            ),
            .stopping
        )
        XCTAssertEqual(
            ConversationLLMActivityState.resolve(
                hasConversation: true,
                streaming: false,
                isCancelling: false
            ),
            .paused
        )
    }

    func testShortReplyStaysExpanded() {
        XCTAssertFalse(AssistantMessageCollapsePolicy.shouldCollapse("A concise reply."))
    }

    func testLongUnbrokenReplyCanCollapse() {
        XCTAssertTrue(
            AssistantMessageCollapsePolicy.shouldCollapse(String(repeating: "长", count: 700))
        )
    }

    func testManyShortLinesCanCollapse() {
        let reply = Array(repeating: "step", count: 22).joined(separator: "\n")
        XCTAssertTrue(AssistantMessageCollapsePolicy.shouldCollapse(reply))
    }

    func testAssistantMarkdownParsesPipeTableAndColumnAlignment() {
        let markdown = """
        | 阶段 | 状态 | Owner |
        | :--- | :---: | ---: |
        | 1. **Design** | 🔄 进行中 | design 子代理 |
        | 2. Dependencies | ⏳ 待执行 | A \\| B |
        """

        XCTAssertEqual(
            AIText.parseBlocks(markdown),
            [
                .table(MDTable(
                    headers: ["阶段", "状态", "Owner"],
                    alignments: [.leading, .center, .trailing],
                    rows: [
                        ["1. **Design**", "🔄 进行中", "design 子代理"],
                        ["2. Dependencies", "⏳ 待执行", "A | B"]
                    ]
                ))
            ]
        )
    }

    func testAssistantMarkdownDoesNotTreatMalformedDividerAsTable() {
        let markdown = """
        Use A | B when comparing values
        | -- | --- |
        """

        XCTAssertEqual(
            AIText.parseBlocks(markdown),
            [
                .text([
                    .paragraph("Use A | B when comparing values"),
                    .paragraph("| -- | --- |")
                ])
            ]
        )
    }

    func testAssistantMarkdownTableAcceptsSingleCellRowsUntilBlockBoundary() {
        let markdown = """
        | Name | Value |
        | --- | --- |
        single cell
        > quote | outside table
        """

        XCTAssertEqual(
            AIText.parseBlocks(markdown),
            [
                .table(MDTable(
                    headers: ["Name", "Value"],
                    alignments: [.leading, .leading],
                    rows: [["single cell", ""]]
                )),
                .text([.paragraph("> quote | outside table")])
            ]
        )
    }

    @MainActor
    func testAssistantMarkdownTableRendersAtPhoneWidth() throws {
        let markdown = """
        | Brief | Details |
        | --- | --- |
        | Short | This deliberately long value wraps across several lines so the neighboring cell must keep its background and divider for the full row height on a narrow phone. |
        """
        let rootView =
            AIText(markdown: markdown)
                .fixedSize(horizontal: false, vertical: true)
                .frame(width: 320, alignment: .leading)
                .padding(16)
                .background(Color.white)
                .environment(\.theme, DesignTokens.dark)
        let controller = UIHostingController(rootView: rootView)
        let fittingSize = controller.sizeThatFits(in: CGSize(width: 352, height: 800))
        let size = CGSize(width: 352, height: fittingSize.height)
        controller.view.bounds = CGRect(origin: .zero, size: size)
        controller.view.backgroundColor = .white
        controller.view.setNeedsLayout()
        controller.view.layoutIfNeeded()

        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = true
        let image = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            controller.view.drawHierarchy(in: controller.view.bounds, afterScreenUpdates: true)
        }
        XCTAssertGreaterThan(image.size.height, 80)
        XCTAssertTrue(Self.containsVisibleContent(image))
        XCTAssertFalse(Self.hasAsymmetricRowFillGap(image))
        let attachment = XCTAttachment(image: image)
        attachment.name = "Assistant-Markdown-Table"
        attachment.lifetime = .deleteOnSuccess
        add(attachment)
    }

    /// The dark table is rendered over a white test background. When one cell
    /// wraps, every scanline covered by its dark fill must also be filled in the
    /// shorter neighboring cell; a sustained mismatch exposes unequal row chrome.
    private static func hasAsymmetricRowFillGap(_ image: UIImage) -> Bool {
        guard let raster = rgbaPixels(in: image) else { return true }
        let leftBand = 28 ..< min(108, raster.width)
        let rightBand = 156 ..< min(236, raster.width)
        guard !leftBand.isEmpty, !rightBand.isEmpty else { return true }

        var mismatchedRun = 0
        for y in 0 ..< raster.height {
            func darkRatio(in band: Range<Int>) -> Double {
                let darkPixels = band.reduce(into: 0) { count, x in
                    let offset = (y * raster.width + x) * 4
                    if raster.pixels[offset] < 240 ||
                        raster.pixels[offset + 1] < 240 ||
                        raster.pixels[offset + 2] < 240 {
                        count += 1
                    }
                }
                return Double(darkPixels) / Double(band.count)
            }

            if darkRatio(in: rightBand) > 0.8, darkRatio(in: leftBand) < 0.2 {
                mismatchedRun += 1
                if mismatchedRun >= 8 { return true }
            } else {
                mismatchedRun = 0
            }
        }
        return false
    }

    private static func containsVisibleContent(_ image: UIImage) -> Bool {
        guard let raster = rgbaPixels(in: image) else { return false }
        for offset in stride(from: 0, to: raster.pixels.count, by: 4) {
            if raster.pixels[offset] < 245 ||
                raster.pixels[offset + 1] < 245 ||
                raster.pixels[offset + 2] < 245 {
                return true
            }
        }
        return false
    }

    private static func rgbaPixels(in image: UIImage) -> (pixels: [UInt8], width: Int, height: Int)? {
        guard let cgImage = image.cgImage else { return nil }
        let width = cgImage.width
        let height = cgImage.height
        var pixels = [UInt8](repeating: 255, count: width * height * 4)

        let rendered = pixels.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(
                data: buffer.baseAddress,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else {
                return false
            }
            context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        return rendered ? (pixels, width, height) : nil
    }

    func testToolIconsUseDifferentActionsForDifferentVerbs() {
        let read = ConversationToolHeader(
            verb: .read,
            label: "Read",
            primary: "README.md",
            qualifier: nil,
            count: nil,
            subLine: nil,
            title: "Read(README.md)"
        )
        let shell = ConversationToolHeader(
            verb: .shell,
            label: "Shell",
            primary: nil,
            qualifier: nil,
            count: nil,
            subLine: nil,
            title: "Shell"
        )

        XCTAssertEqual(ToolDisplayText.icon(header: read, tool: "Read"), .bookOpen)
        XCTAssertEqual(ToolDisplayText.icon(header: shell, tool: "Shell"), .terminal)
        XCTAssertNotEqual(
            ToolDisplayText.icon(header: read, tool: "Read"),
            ToolDisplayText.icon(header: shell, tool: "Shell")
        )
    }

    func testLegacyToolNamesStillGetSpecificIcons() {
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "WebSearch"), .globe)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "bash"), .terminal)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "Write"), .pencil)
        XCTAssertEqual(ToolDisplayText.icon(header: nil, tool: "Search documentation"), .search)
    }

    func testStructuredToolExpansionKeysAreScopedToTheirMessage() {
        let firstMessage = UUID()
        let secondMessage = UUID()
        let firstKey = ConversationToolExpansionKey.structured(
            messageID: firstMessage,
            toolID: "read-1"
        )
        let secondKey = ConversationToolExpansionKey.structured(
            messageID: secondMessage,
            toolID: "read-1"
        )

        XCTAssertNotEqual(firstKey, secondKey)
        XCTAssertEqual(
            ConversationToolExpansionKey.structuredToolIDs(
                in: [firstKey, secondKey, "standalone-tool"],
                messageID: firstMessage
            ),
            ["read-1"]
        )
    }
}
