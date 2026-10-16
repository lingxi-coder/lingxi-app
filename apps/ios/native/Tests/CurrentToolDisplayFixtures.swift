#if canImport(harness_runtimeFFI)
import harness_runtimeFFI

func currentToolHeader(_ label: String = "Read") -> ToolHeaderDto {
    ToolHeaderDto(
        verb: .generic,
        icon: nil,
        label: label,
        primary: nil,
        qualifier: nil,
        count: nil,
        subLine: nil,
        title: label
    )
}

func currentToolDisplay(
    headline: String? = nil,
    body: String? = nil
) -> ToolResultDisplayDto {
    ToolResultDisplayDto(
        headline: headline,
        headlineKind: nil,
        headlineArgs: [],
        diff: nil,
        body: body,
        bodyLines: 0,
        bodyTruncated: false,
        collapsed: false
    )
}
#endif
