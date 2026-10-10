package com.lingxi.code.conversation

import androidx.compose.runtime.Immutable
import com.lingxi.code.bindings.client.VisualizationBlockStatusDto
import com.lingxi.code.bindings.client.VisualizationRefDto
import com.lingxi.code.model.Message

/** One published revision of an inline visualization. */
@Immutable
data class VisualizationRef(val id: String, val revision: UInt)

/** How a settled visualization slot renders. */
enum class VisualizationSlotStatus { Pending, Ready, Unavailable }

/** Live progress of a slot, as the engine streams it. */
enum class VisualizationBlockStatus { Pending, Ready, Unavailable, Discarded }

/** The widget a user message continues from, shown as an attachment chip. */
@Immutable
data class VisualizationContextChip(val id: String, val revision: UInt, val title: String) {
    val reference: VisualizationRef get() = VisualizationRef(id, revision)
}

/** A follow-up question a widget drafted for the composer. */
@Immutable
data class VisualizationFollowup(val text: String, val chip: VisualizationContextChip)

internal fun VisualizationRefDto.toUi(): VisualizationRef = VisualizationRef(id, revision)

internal fun VisualizationRef.toDto(): VisualizationRefDto = VisualizationRefDto(id = id, revision = revision)

internal fun VisualizationBlockStatusDto.toUi(): VisualizationBlockStatus = when (this) {
    VisualizationBlockStatusDto.PENDING -> VisualizationBlockStatus.Pending
    VisualizationBlockStatusDto.READY -> VisualizationBlockStatus.Ready
    VisualizationBlockStatusDto.UNAVAILABLE -> VisualizationBlockStatus.Unavailable
    VisualizationBlockStatusDto.DISCARDED -> VisualizationBlockStatus.Discarded
}

/** The retained-journal spelling of [VisualizationBlockStatus]. */
internal fun visualizationBlockStatus(wire: String): VisualizationBlockStatus? = when (wire) {
    "pending" -> VisualizationBlockStatus.Pending
    "ready" -> VisualizationBlockStatus.Ready
    "unavailable" -> VisualizationBlockStatus.Unavailable
    "discarded" -> VisualizationBlockStatus.Discarded
    else -> null
}

/**
 * Append streamed prose. A message no visualization has split keeps its prose
 * in [Message.text] alone; once one has, prose continues in a trailing text
 * block so it renders after the widget instead of above it.
 */
internal fun Message.appendingStreamText(delta: String): Message {
    if (blocks.isEmpty()) return copy(text = text + delta)
    val last = blocks.last()
    val next = if (last is MessageContent.Text) {
        blocks.dropLast(1) + MessageContent.Text(last.text + delta)
    } else {
        blocks + MessageContent.Text(delta)
    }
    return copy(text = text + delta, blocks = next)
}

/**
 * Fold one live slot update into the streaming message. A pending slot
 * settles in place; a discarded one disappears. The prose streamed so far is
 * seeded as a block first, because a message with blocks renders only those.
 */
internal fun Message.applyingVisualizationBlock(
    status: VisualizationBlockStatus,
    reference: VisualizationRef?,
): Message {
    val seeded = blocks.ifEmpty { if (text.isBlank()) emptyList() else listOf(MessageContent.Text(text)) }
    val pendingAt = seeded.indexOfLast {
        it is MessageContent.Visualization && it.status == VisualizationSlotStatus.Pending
    }
    fun settle(slot: MessageContent.Visualization): List<MessageContent> =
        if (pendingAt >= 0) seeded.toMutableList().also { it[pendingAt] = slot } else seeded + slot
    val next = when (status) {
        VisualizationBlockStatus.Pending ->
            if (pendingAt >= 0) seeded
            else seeded + MessageContent.Visualization(VisualizationSlotStatus.Pending, null)
        VisualizationBlockStatus.Ready -> settle(
            if (reference != null) MessageContent.Visualization(VisualizationSlotStatus.Ready, reference)
            else MessageContent.Visualization(VisualizationSlotStatus.Unavailable, null),
        )
        VisualizationBlockStatus.Unavailable ->
            settle(MessageContent.Visualization(VisualizationSlotStatus.Unavailable, null))
        VisualizationBlockStatus.Discarded ->
            if (pendingAt >= 0) seeded.filterIndexed { index, _ -> index != pendingAt } else seeded
    }
    return copy(blocks = next)
}

/** A turn that ends with a slot still pending leaves no placeholder behind. */
internal fun Message.withoutPendingVisualizations(): Message {
    val pending = { block: MessageContent ->
        block is MessageContent.Visualization && block.status == VisualizationSlotStatus.Pending
    }
    return if (blocks.none(pending)) this else copy(blocks = blocks.filterNot(pending))
}
