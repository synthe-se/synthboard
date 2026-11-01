package app.synthboard

import android.content.Context
import android.graphics.Canvas
import android.graphics.ColorMatrix
import android.graphics.ColorMatrixColorFilter
import android.graphics.Paint
import android.graphics.RectF
import android.view.GestureDetector
import android.view.MotionEvent
import android.widget.OverScroller
import app.synthboard.core.EmojiItem
import app.synthboard.core.Event
import app.synthboard.core.Icon
import app.synthboard.core.LETTERS_LABEL

/**
 * The scrolling emoji panel inside the keyboard view. Long press relies on
 * `GestureDetector`'s own timeout: this one framework timer is the price of
 * letting Android tell a scroll from a press, and it runs only in this panel.
 */
class EmojiGrid(
    context: Context,
    private val density: Float,
    private val palette: () -> Palette,
    private val recentsTitle: String,
    private val searchHint: String,
    private val invalidate: () -> Unit,
    private val send: (Event) -> Unit,
) {
    private sealed interface Row
    private data class Header(val title: String) : Row
    private data class Cells(val items: List<EmojiItem>) : Row

    private val cell = CELL_DP * density
    private val header = HEADER_DP * density
    private var bar = 0f
    private val strip = STRIP_DP * density
    private val scroller = OverScroller(context)

    private val emojiPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
        textSize = EMOJI_SP * density
    }
    private val greyPaint = Paint(emojiPaint).apply {
        textSize = STRIP_EMOJI_SP * density
        colorFilter = ColorMatrixColorFilter(ColorMatrix().apply { setSaturation(0f) })
    }
    private val labelPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textSize = LABEL_SP * density
    }
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val iconPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        strokeWidth = ICON_STROKE_DP * density
    }
    private val rect = RectF()

    private var groups: List<Pair<String, List<EmojiItem>>> = emptyList()
    private var recents: List<String> = emptyList()
    private var rows: List<Row> = emptyList()
    private var rowTops = FloatArray(0)
    private var contentHeight = 0f
    private var width = 0
    private var height = 0
    private var scrollY = 0f
    private var tone = 0
    private var tonePopup: List<String> = emptyList()
    private var toneAnchorY = 0f
    private var toneX = 0f
    private var toneSelected = -1

    private val detector = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            return true
        }

        override fun onScroll(first: MotionEvent?, e: MotionEvent, dx: Float, dy: Float): Boolean {
            scrollBy(dy)
            return true
        }

        override fun onFling(first: MotionEvent?, e: MotionEvent, vx: Float, vy: Float): Boolean {
            // Timer justified: a fling is motion in time, driven by the frames.
            scroller.fling(0, scrollY.toInt(), 0, -vy.toInt(), 0, 0, 0, maxScroll().toInt())
            invalidate()
            return true
        }

        override fun onSingleTapUp(e: MotionEvent): Boolean {
            tap(e.x, e.y)
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            openTones(e.x, e.y)
        }
    })

    fun setGroups(loaded: List<Pair<String, List<EmojiItem>>>) {
        groups = loaded
        rebuild()
    }

    fun open(recent: List<String>, savedTone: Int) {
        recents = recent
        tone = savedTone
        scrollY = 0f
        rebuild()
    }

    fun resize(newWidth: Int, newHeight: Int, barHeight: Float) {
        width = newWidth
        height = newHeight
        bar = barHeight
        rebuild()
    }

    private fun rebuild() {
        val columns = (width / cell).toInt().coerceAtLeast(1)
        val built = ArrayList<Row>()
        if (recents.isNotEmpty()) {
            built += Header(recentsTitle)
            recents.map { EmojiItem(it, emptyList()) }.chunked(columns).forEach { built += Cells(it) }
        }
        for ((name, items) in groups) {
            built += Header(name)
            items.chunked(columns).forEach { built += Cells(it) }
        }
        rows = built
        rowTops = FloatArray(built.size)
        var y = 0f
        built.forEachIndexed { index, row ->
            rowTops[index] = y
            y += if (row is Header) header else cell
        }
        contentHeight = y
    }

    private fun gridBottom() = height - strip

    private fun maxScroll() = (contentHeight - (gridBottom() - bar)).coerceAtLeast(0f)

    private fun scrollBy(dy: Float) {
        scrollY = (scrollY + dy).coerceIn(0f, maxScroll())
        invalidate()
    }

    /** The five skin tones come first in an item's variants, then its genders. */
    private fun shown(item: EmojiItem): String =
        if (tone > 0 && item.tones.size >= SKIN_TONES) item.tones[tone - 1] else item.emoji

    private fun cellAt(x: Float, y: Float): EmojiItem? {
        if (y < bar || y >= gridBottom()) return null
        val contentY = y - bar + scrollY
        val index = rowTops.indexOfLast { it <= contentY }
        val row = rows.getOrNull(index) as? Cells ?: return null
        return row.items.getOrNull((x / cell).toInt())
    }

    private fun tap(x: Float, y: Float) {
        when {
            y < bar -> send(Event.OpenEmojiSearch)
            y >= gridBottom() -> tapStrip(x)
            else -> cellAt(x, y)?.let { send(Event.Text(shown(it))) }
        }
    }

    private fun stripSlots() = groups.size + 2

    private fun tapStrip(x: Float) {
        val slot = (x / (width.toFloat() / stripSlots())).toInt()
        when {
            slot == 0 -> send(Event.CloseEmojiPanel)
            slot >= stripSlots() - 1 -> send(Event.DeleteOne)
            else -> jumpTo(groups[slot - 1].first)
        }
    }

    private fun jumpTo(name: String) {
        val index = rows.indexOfFirst { it is Header && it.title == name }
        if (index >= 0) {
            scroller.forceFinished(true)
            scrollY = rowTops[index].coerceIn(0f, maxScroll())
            invalidate()
        }
    }

    private fun openTones(x: Float, y: Float) {
        val item = cellAt(x, y) ?: return
        if (item.tones.isEmpty()) return
        tonePopup = listOf(item.emoji) + item.tones
        toneAnchorY = (y - cell * TONE_LIFT_CELLS).coerceAtLeast(bar)
        toneX = (x - cell).coerceIn(0f, (width - cell * tonePopup.size).coerceAtLeast(0f))
        toneSelected = (tone).coerceIn(0, tonePopup.lastIndex)
        invalidate()
    }

    fun onTouch(event: MotionEvent): Boolean {
        if (tonePopup.isNotEmpty()) {
            when (event.actionMasked) {
                MotionEvent.ACTION_MOVE -> {
                    toneSelected = ((event.x - toneX) / cell).toInt().coerceIn(0, tonePopup.lastIndex)
                    invalidate()
                }
                MotionEvent.ACTION_UP -> {
                    send(Event.Text(tonePopup[toneSelected]))
                    if (toneSelected <= SKIN_TONES && tonePopup.size > SKIN_TONES) {
                        tone = toneSelected
                        onToneChosen(tone)
                    }
                    tonePopup = emptyList()
                    invalidate()
                }
                MotionEvent.ACTION_CANCEL -> {
                    tonePopup = emptyList()
                    invalidate()
                }
            }
            return true
        }
        return detector.onTouchEvent(event)
    }

    var onToneChosen: (Int) -> Unit = {}

    fun draw(canvas: Canvas) {
        labelPaint.color = palette().text
        iconPaint.color = palette().text
        if (scroller.computeScrollOffset()) {
            scrollY = scroller.currY.toFloat().coerceIn(0f, maxScroll())
            invalidate()
        }
        drawSearchRow(canvas)
        canvas.save()
        canvas.clipRect(0f, bar, width.toFloat(), gridBottom())
        val baseline = cell / 2 - (emojiPaint.descent() + emojiPaint.ascent()) / 2
        rows.forEachIndexed { index, row ->
            val top = bar + rowTops[index] - scrollY
            val rowHeight = if (row is Header) header else cell
            if (top + rowHeight < bar || top > gridBottom()) return@forEachIndexed
            when (row) {
                is Header -> canvas.drawText(row.title, cell / 4, top + header * HEADER_BASELINE, labelPaint)
                is Cells -> row.items.forEachIndexed { column, item ->
                    canvas.drawText(shown(item), cell * column + cell / 2, top + baseline, emojiPaint)
                }
            }
        }
        canvas.restore()
        drawStrip(canvas)
        drawTones(canvas)
    }

    private fun drawSearchRow(canvas: Canvas) {
        Icons.drawSearch(canvas, bar / 2, bar / 2, ICON_DP * density, iconPaint)
        canvas.drawText(searchHint, bar, bar / 2 - (labelPaint.descent() + labelPaint.ascent()) / 2, labelPaint)
    }

    private fun drawStrip(canvas: Canvas) {
        val slot = width.toFloat() / stripSlots()
        val center = gridBottom() + strip / 2
        val text = LETTERS_LABEL
        val textWidth = labelPaint.measureText(text)
        canvas.drawText(text, slot / 2 - textWidth / 2, center - (labelPaint.descent() + labelPaint.ascent()) / 2, labelPaint)
        groups.forEachIndexed { index, (_, items) ->
            val icon = items.firstOrNull()?.emoji ?: return@forEachIndexed
            canvas.drawText(icon, slot * (index + 1.5f), center - (greyPaint.descent() + greyPaint.ascent()) / 2, greyPaint)
        }
        Icons.draw(canvas, Icon.DELETE, slot * (stripSlots() - 0.5f), center, ICON_DP * density, iconPaint)
    }

    private fun drawTones(canvas: Canvas) {
        if (tonePopup.isEmpty()) return
        rect.set(toneX, toneAnchorY, toneX + cell * tonePopup.size, toneAnchorY + cell)
        fill.color = palette().key
        canvas.drawRoundRect(rect, KEY_RADIUS_DP * density, KEY_RADIUS_DP * density, fill)
        val baseline = toneAnchorY + cell / 2 - (emojiPaint.descent() + emojiPaint.ascent()) / 2
        tonePopup.forEachIndexed { index, emoji ->
            val left = toneX + cell * index
            if (index == toneSelected) {
                rect.set(left, toneAnchorY, left + cell, toneAnchorY + cell)
                fill.color = palette().pressed
                canvas.drawRoundRect(rect, KEY_RADIUS_DP * density, KEY_RADIUS_DP * density, fill)
            }
            canvas.drawText(emoji, left + cell / 2, baseline, emojiPaint)
        }
    }

    private companion object {
        const val SKIN_TONES = 5

        /** How far above the finger the skin tones open, in cells. */
        const val TONE_LIFT_CELLS = 1.5f

        /** Where a group title sits in its header row. */
        const val HEADER_BASELINE = 0.72f
        const val CELL_DP = 48f
        const val HEADER_DP = 28f
        const val STRIP_DP = 44f
        const val EMOJI_SP = 28f
        const val STRIP_EMOJI_SP = 18f
        const val LABEL_SP = 13f
        const val ICON_DP = 22f
        const val ICON_STROKE_DP = 1.8f
    }
}
