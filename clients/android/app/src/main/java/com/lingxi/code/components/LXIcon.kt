package com.lingxi.code.components

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.RoundRect
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * LXIcon — the brand icon set, ported 1:1 from the iOS `LXIcon.swift` (itself a
 * port of the prototype's inline-SVG `<Icon name=…>` set). Each icon draws the
 * same 24×24-viewBox path data, scaled to [size], on a Compose [Canvas].
 *
 * Stroke icons use round caps/joins to match the original
 * `strokeLinecap/Linejoin="round"`; a few glyphs (arrowUp/play/pause) are
 * filled. This keeps the icon library self-contained and brand-exact rather
 * than approximating with Material icons.
 */
enum class LXIconName {
    Menu, Edit, Search, Sparkle, Book, Workflow, Cog, Plus, Mic, Paperclip,
    Chevron, Sun, Moon, Check, Pin, Brain, ArrowUp, Folder, Clock, Message,
    ChevronR, Play, Pause, X, Skill, Plug, Dream, Link, Copy, Share,
    ArrowRight,
}

@Composable
fun LXIcon(
    name: LXIconName,
    modifier: Modifier = Modifier,
    size: Dp = 20.dp,
    color: Color = Color.White,
    stroke: Float = 1.7f,
    /**
     * Accessibility label. When non-null the glyph becomes a labeled
     * a11y node (TalkBack reads it); when null the glyph stays decorative
     * (the default for icons that sit inside an already-labeled control or
     * accompany a visible text label). Icon-only buttons pass a label here.
     */
    contentDescription: String? = null,
) {
    val a11y = if (contentDescription != null) {
        Modifier.clearAndSetSemantics { this.contentDescription = contentDescription }
    } else {
        Modifier
    }
    Canvas(modifier = modifier.then(a11y).size(size)) {
        val s = this.size.width / 24f // scale from the 24-unit viewBox
        val strokeStyle = Stroke(
            width = stroke * s,
            cap = StrokeCap.Round,
            join = StrokeJoin.Round,
        )
        scale(scaleX = s, scaleY = s, pivot = Offset.Zero) {
            for (build in strokePaths(name)) {
                drawPath(build(), color = color, style = strokeStyle)
            }
            for (build in fillPaths(name)) {
                drawPath(build(), color = color)
            }
        }
    }
}

// ---- Path primitives (24-unit viewBox) ------------------------------------

private fun line(pts: List<Pair<Float, Float>>): Path {
    val p = Path()
    val first = pts.firstOrNull() ?: return p
    p.moveTo(first.first, first.second)
    for (pt in pts.drop(1)) p.lineTo(pt.first, pt.second)
    return p
}

private fun circle(cx: Float, cy: Float, r: Float): Path =
    Path().apply { addOval(Rect(cx - r, cy - r, cx + r, cy + r)) }

private fun roundedRect(x: Float, y: Float, w: Float, h: Float, r: Float): Path =
    Path().apply {
        addRoundRect(RoundRect(Rect(x, y, x + w, y + h), CornerRadius(r, r)))
    }

/** Cubic-curve path builder mirroring SwiftUI's `addCurve(to:control1:control2:)`. */
private class Pen {
    val path = Path()
    fun move(x: Float, y: Float) = apply { path.moveTo(x, y) }
    fun line(x: Float, y: Float) = apply { path.lineTo(x, y) }
    fun curve(x: Float, y: Float, c1x: Float, c1y: Float, c2x: Float, c2y: Float) =
        apply { path.cubicTo(c1x, c1y, c2x, c2y, x, y) }
    fun close() = apply { path.close() }
    fun build(): Path = path
}

private inline fun pen(block: Pen.() -> Unit): Path = Pen().apply(block).build()

// ---- Stroke + fill path tables --------------------------------------------

private fun strokePaths(name: LXIconName): List<() -> Path> = when (name) {
    LXIconName.Menu -> listOf(
        { line(listOf(3f to 6f, 21f to 6f)) },
        { line(listOf(3f to 12f, 21f to 12f)) },
        { line(listOf(3f to 18f, 21f to 18f)) },
    )
    LXIconName.Edit -> listOf(
        { line(listOf(12f to 20f, 21f to 20f)) },
        {
            pen {
                move(16.5f, 3.5f)
                curve(19.5f, 6.5f, 17.66f, 3.5f, 19.5f, 5.34f)
                line(7f, 19f); line(3f, 20f); line(4f, 16f); close()
            }
        },
    )
    LXIconName.Search -> listOf(
        { circle(11f, 11f, 7f) },
        { line(listOf(21f to 21f, 16.7f to 16.7f)) },
    )
    LXIconName.Sparkle -> listOf(
        { line(listOf(12f to 3f, 12f to 6f)) },
        { line(listOf(12f to 18f, 12f to 21f)) },
        { line(listOf(3f to 12f, 6f to 12f)) },
        { line(listOf(18f to 12f, 21f to 12f)) },
        { line(listOf(5.6f to 5.6f, 7.7f to 7.7f)) },
        { line(listOf(16.3f to 16.3f, 18.4f to 18.4f)) },
        { line(listOf(5.6f to 18.4f, 7.7f to 16.3f)) },
        { line(listOf(16.3f to 7.7f, 18.4f to 5.6f)) },
        { circle(12f, 12f, 2.5f) },
    )
    LXIconName.Book -> listOf(
        {
            pen {
                move(4f, 19.5f)
                curve(6.5f, 17f, 4f, 18.12f, 5.12f, 17f)
                line(20f, 17f)
            }
        },
        {
            pen {
                move(6.5f, 2f); line(20f, 2f); line(20f, 22f); line(6.5f, 22f)
                curve(4f, 19.5f, 5.12f, 22f, 4f, 20.88f)
                line(4f, 4.5f)
                curve(6.5f, 2f, 4f, 3.12f, 5.12f, 2f); close()
            }
        },
    )
    LXIconName.Workflow -> listOf(
        { roundedRect(3f, 3f, 6f, 6f, 1.5f) },
        { roundedRect(15f, 3f, 6f, 6f, 1.5f) },
        { roundedRect(9f, 15f, 6f, 6f, 1.5f) },
        {
            pen {
                move(6f, 9f); line(6f, 11f)
                curve(8f, 13f, 6f, 12.1f, 6.9f, 13f)
                line(16f, 13f)
                curve(18f, 11f, 17.1f, 13f, 18f, 12.1f)
                line(18f, 9f)
            }
        },
    )
    LXIconName.Cog -> listOf(
        { circle(12f, 12f, 3f) },
        {
            pen {
                move(19.4f, 15f)
                curve(19.73f, 16.82f, 19.18f, 15.66f, 19.3f, 16.39f)
                line(19.79f, 16.88f)
                curve(16.96f, 19.71f, 20.57f, 17.66f, 17.74f, 20.49f)
                line(16.9f, 19.65f)
                curve(15.08f, 19.32f, 16.61f, 19.36f, 15.74f, 19.1f)
                curve(14.08f, 20.83f, 14.42f, 19.54f, 14.08f, 20.13f)
                line(14.08f, 21f)
                curve(10.08f, 21f, 14.08f, 22.1f, 10.08f, 22.1f)
                line(10.08f, 20.91f)
                curve(9.08f, 19.4f, 10.08f, 20.17f, 9.74f, 19.62f)
                curve(7.26f, 19.73f, 8.42f, 19.18f, 7.69f, 19.3f)
                line(7.2f, 19.79f)
                curve(4.37f, 16.96f, 6.42f, 20.57f, 3.59f, 17.74f)
                line(4.43f, 16.9f)
                curve(4.76f, 15.08f, 4.72f, 16.61f, 4.98f, 15.74f)
                curve(3.25f, 14.08f, 4.54f, 14.42f, 3.95f, 14.08f)
                line(3f, 14.08f)
                curve(3f, 10.08f, 1.9f, 14.08f, 1.9f, 10.08f)
                line(3.09f, 10.08f)
                curve(4.6f, 9.08f, 3.83f, 10.08f, 4.38f, 9.74f)
                curve(4.27f, 7.26f, 4.82f, 8.42f, 4.7f, 7.69f)
                line(4.21f, 7.2f)
                curve(7.04f, 4.37f, 3.43f, 6.42f, 6.26f, 3.59f)
                line(7.1f, 4.43f)
                curve(8.92f, 4.76f, 7.39f, 4.72f, 8.26f, 4.98f)
                curve(9.92f, 3.25f, 9.58f, 4.54f, 9.92f, 3.95f)
                line(9.92f, 3f)
                curve(13.92f, 3f, 9.92f, 1.9f, 13.92f, 1.9f)
                line(13.92f, 3.09f)
                curve(14.92f, 4.6f, 13.92f, 3.83f, 14.26f, 4.38f)
                curve(16.74f, 4.27f, 15.58f, 4.82f, 16.31f, 4.7f)
                line(16.8f, 4.21f)
                curve(19.63f, 7.04f, 17.58f, 3.43f, 20.41f, 6.26f)
                line(19.57f, 7.1f)
                curve(19.24f, 8.92f, 19.28f, 7.39f, 19.02f, 8.26f)
                line(19.24f, 9f)
                curve(20.75f, 10f, 19.46f, 9.66f, 20.05f, 10f)
                line(21f, 10f)
                curve(21f, 14f, 22.1f, 10f, 22.1f, 14f)
                line(20.91f, 14f)
                curve(19.4f, 15f, 20.17f, 14f, 19.62f, 14.34f); close()
            }
        },
    )
    LXIconName.Plus -> listOf(
        { line(listOf(12f to 5f, 12f to 19f)) },
        { line(listOf(5f to 12f, 19f to 12f)) },
    )
    LXIconName.Mic -> listOf(
        { roundedRect(9f, 2f, 6f, 11f, 3f) },
        {
            pen {
                move(19f, 10f); line(19f, 12f)
                curve(5f, 12f, 19f, 15.87f, 5f, 15.87f)
                line(5f, 10f)
            }
        },
        { line(listOf(12f to 19f, 12f to 22f)) },
    )
    LXIconName.Paperclip -> listOf(
        {
            pen {
                move(21.44f, 11.05f); line(12.25f, 20.24f)
                curve(3.76f, 11.75f, 9.9f, 22.59f, 6.11f, 22.59f)
                line(12.33f, 3.18f)
                curve(17.93f, 8.83f, 13.9f, 1.61f, 19.5f, 7.26f)
                line(9.34f, 17.4f)
                curve(6.51f, 14.57f, 8.56f, 18.18f, 5.73f, 15.35f)
                line(15f, 6.09f)
            }
        },
    )
    LXIconName.Chevron -> listOf({ line(listOf(6f to 9f, 12f to 15f, 18f to 9f)) })
    LXIconName.ChevronR -> listOf({ line(listOf(9f to 18f, 15f to 12f, 9f to 6f)) })
    LXIconName.ArrowRight -> listOf(
        { line(listOf(5f to 12f, 19f to 12f)) },
        { line(listOf(12f to 5f, 19f to 12f, 12f to 19f)) },
    )
    LXIconName.X -> listOf(
        { line(listOf(18f to 6f, 6f to 18f)) },
        { line(listOf(6f to 6f, 18f to 18f)) },
    )
    LXIconName.Sun -> listOf(
        { circle(12f, 12f, 4f) },
        { line(listOf(12f to 2f, 12f to 4f)) },
        { line(listOf(12f to 20f, 12f to 22f)) },
        { line(listOf(4.93f to 4.93f, 6.34f to 6.34f)) },
        { line(listOf(17.66f to 17.66f, 19.07f to 19.07f)) },
        { line(listOf(2f to 12f, 4f to 12f)) },
        { line(listOf(20f to 12f, 22f to 12f)) },
        { line(listOf(6.34f to 17.66f, 4.93f to 19.07f)) },
        { line(listOf(19.07f to 4.93f, 17.66f to 6.34f)) },
    )
    LXIconName.Moon, LXIconName.Dream -> listOf(
        {
            pen {
                move(21f, 12.79f)
                curve(11.21f, 3f, 20.27f, 18.2f, 15.27f, 3.73f)
                curve(21f, 12.79f, 14.73f, 3.73f, 20.27f, 9.6f); close()
            }
        },
    )
    LXIconName.Check -> listOf({ line(listOf(20f to 6f, 9f to 17f, 4f to 12f)) })
    LXIconName.Pin -> listOf(
        { line(listOf(12f to 17f, 12f to 22f)) },
        {
            pen {
                move(9f, 10.76f); line(9f, 3f); line(15f, 3f); line(15f, 10.76f)
                line(18f, 12.7f); line(18f, 16f); line(6f, 16f); line(6f, 12.7f); close()
            }
        },
    )
    LXIconName.Brain -> listOf(
        {
            pen {
                move(9.5f, 2f)
                curve(12f, 4.5f, 10.88f, 2f, 12f, 3.12f)
                line(12f, 19.5f)
                curve(7.04f, 19.94f, 12f, 20.88f, 8.5f, 21.5f)
                curve(4.5f, 17f, 5.5f, 19.5f, 4.5f, 18.4f)
                curve(2.5f, 13.35f, 3.4f, 16.6f, 2.5f, 14.6f)
                curve(4f, 8.5f, 2.8f, 11.2f, 3f, 9.3f)
                curve(6.5f, 6f, 4f, 7.12f, 5.12f, 6f)
                curve(9.5f, 2f, 6.5f, 3.79f, 7.5f, 2f); close()
            }
        },
        {
            pen {
                move(14.5f, 2f)
                curve(12f, 4.5f, 13.12f, 2f, 12f, 3.12f)
                line(12f, 19.5f)
                curve(16.96f, 19.94f, 12f, 20.88f, 15.5f, 21.5f)
                curve(19.5f, 17f, 18.5f, 19.5f, 19.5f, 18.4f)
                curve(21.5f, 13.35f, 20.6f, 16.6f, 21.5f, 14.6f)
                curve(20f, 8.5f, 21.2f, 11.2f, 21f, 9.3f)
                curve(17.5f, 6f, 20f, 7.12f, 18.88f, 6f)
                curve(14.5f, 2f, 17.5f, 3.79f, 16.5f, 2f); close()
            }
        },
    )
    LXIconName.Folder -> listOf(
        {
            pen {
                move(3f, 7f)
                curve(5f, 5f, 3f, 5.9f, 3.9f, 5f)
                line(9f, 5f); line(11f, 7f); line(19f, 7f)
                curve(21f, 9f, 20.1f, 7f, 21f, 7.9f)
                line(21f, 18f)
                curve(19f, 20f, 21f, 19.1f, 20.1f, 20f)
                line(5f, 20f)
                curve(3f, 18f, 3.9f, 20f, 3f, 19.1f); close()
            }
        },
    )
    LXIconName.Clock -> listOf(
        { circle(12f, 12f, 9f) },
        { line(listOf(12f to 7f, 12f to 12f, 15f to 14f)) },
    )
    LXIconName.Message -> listOf(
        {
            pen {
                move(21f, 11.5f)
                curve(20.1f, 15.3f, 21f, 12.83f, 20.69f, 14.13f)
                curve(12.5f, 20f, 18.65f, 18.18f, 15.7f, 20f)
                curve(8.7f, 19.1f, 11.17f, 20f, 9.87f, 19.69f)
                line(3f, 21f); line(4.9f, 15.3f)
                curve(4f, 11.5f, 4.31f, 14.13f, 4f, 12.83f)
                curve(8.7f, 3.9f, 4f, 8.3f, 5.82f, 5.35f)
                curve(12.5f, 3f, 9.87f, 3.31f, 11.17f, 3f)
                line(13f, 3f)
                curve(21f, 11f, 17.39f, 3.25f, 20.75f, 6.61f); close()
            }
        },
    )
    LXIconName.Skill -> listOf(
        { circle(12f, 8f, 3.5f) },
        {
            pen {
                move(5f, 21f); line(5f, 19f)
                curve(9f, 15f, 5f, 16.79f, 6.79f, 15f)
                line(15f, 15f)
                curve(19f, 19f, 17.21f, 15f, 19f, 16.79f)
                line(19f, 21f)
            }
        },
        { line(listOf(19f to 4f, 20f to 5f, 19f to 6f)) },
        { line(listOf(5f to 4f, 4f to 5f, 5f to 6f)) },
    )
    LXIconName.Plug -> listOf(
        { line(listOf(9f to 2f, 9f to 8f)) },
        { line(listOf(15f to 2f, 15f to 8f)) },
        {
            pen {
                move(6f, 8f); line(18f, 8f); line(18f, 11f)
                curve(6f, 11f, 18f, 14.31f, 6f, 14.31f); close()
            }
        },
        { line(listOf(12f to 17f, 12f to 22f)) },
    )
    LXIconName.Link -> listOf(
        {
            pen {
                move(10f, 13f)
                curve(13.5f, 14.5f, 10.79f, 14.06f, 12.27f, 14.79f)
                line(18f, 10f)
                curve(14f, 6f, 20.21f, 7.79f, 16.21f, 3.79f)
                line(11.5f, 8.5f)
            }
        },
        {
            pen {
                move(14f, 11f)
                curve(10.5f, 9.5f, 13.21f, 9.94f, 11.73f, 9.21f)
                line(6f, 14f)
                curve(10f, 18f, 3.79f, 16.21f, 7.79f, 20.21f)
                line(12.5f, 15.5f)
            }
        },
    )
    LXIconName.Copy -> listOf(
        { roundedRect(9f, 9f, 13f, 13f, 2f) },
        {
            pen {
                move(5f, 15f); line(4f, 15f)
                curve(2f, 13f, 2.9f, 15f, 2f, 14.1f)
                line(2f, 4f)
                curve(4f, 2f, 2f, 2.9f, 2.9f, 2f)
                line(13f, 2f)
                curve(15f, 4f, 14.1f, 2f, 15f, 2.9f)
                line(15f, 5f)
            }
        },
    )
    // Share — three nodes connected by two edges (the classic share glyph).
    LXIconName.Share -> listOf(
        { circle(18f, 5f, 3f) },
        { circle(6f, 12f, 3f) },
        { circle(18f, 19f, 3f) },
        { line(listOf(8.59f to 13.51f, 15.42f to 17.49f)) },
        { line(listOf(15.41f to 6.51f, 8.59f to 10.49f)) },
    )
    // Filled glyphs — no stroke geometry.
    LXIconName.ArrowUp, LXIconName.Play, LXIconName.Pause -> emptyList()
}

private fun fillPaths(name: LXIconName): List<() -> Path> = when (name) {
    LXIconName.ArrowUp -> listOf({
        line(listOf(12f to 4f, 5f to 12f, 9f to 12f, 9f to 20f, 15f to 20f, 15f to 12f, 19f to 12f))
            .apply { close(); fillType = PathFillType.NonZero }
    })
    LXIconName.Play -> listOf({
        line(listOf(6f to 4f, 20f to 12f, 6f to 20f)).apply { close() }
    })
    LXIconName.Pause -> listOf(
        { roundedRect(6f, 5f, 4f, 14f, 1f) },
        { roundedRect(14f, 5f, 4f, 14f, 1f) },
    )
    else -> emptyList()
}
