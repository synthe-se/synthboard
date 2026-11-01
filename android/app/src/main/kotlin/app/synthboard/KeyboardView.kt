package app.synthboard

import android.annotation.SuppressLint
import android.content.Context
import android.content.res.Configuration
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import android.view.WindowInsets
import app.synthboard.core.Command
import app.synthboard.core.DrawnKey
import app.synthboard.core.EmojiItem
import app.synthboard.core.Event
import app.synthboard.core.Icon
import app.synthboard.core.POPUP_HEIGHT_DP
import app.synthboard.core.SEARCH_CELLS
import app.synthboard.core.SLOT_CENTERS
import app.synthboard.core.ShiftLook

/** The corners of keys and of everything drawn like them. */
internal const val KEY_RADIUS_DP = 8f

/** Draws what the core describes and reports touches. It decides nothing. */
@SuppressLint("ViewConstructor")
class KeyboardView(context: Context, private val send: (Event) -> Unit) : View(context) {
    private val density = resources.displayMetrics.density
    private var palette = Palette(context)

    init {
        filterTouchesWhenObscured = true
        setAccessibilityDataSensitive(ACCESSIBILITY_DATA_SENSITIVE_YES)
        accessibilityLiveRegion = ACCESSIBILITY_LIVE_REGION_POLITE
    }

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER }
    private val trailPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        strokeWidth = TRAIL_DP * density
    }
    private val iconPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        strokeWidth = ICON_STROKE_DP * density
    }
    private val rect = RectF()
    private val emojiGrid = EmojiGrid(
        context,
        density,
        { palette },
        context.getString(R.string.recents),
        context.getString(R.string.search),
        ::invalidate,
        send,
    )

    private var panelOpen = false
    private var searching = false
    private var undoOffered = false
    private var searchQuery = ""
    private var searchResults: List<String> = emptyList()

    private var keys: List<DrawnKey> = emptyList()
    private var boardHeight = 0f
    private var barHeight = 0f
    private var knownWidth = 0
    private var bottomInset = 0
    private var pressed = -1
    private var shiftLook = ShiftLook.OFF
    private var popup: Command.Popup? = null
    private var suggestions = listOf("", "", "")

    private val trail = Path()
    private var trailActive = false
    private var preview = ""
    var onGestureMoved: () -> Unit = {}
    var showTrail = true
    private var trailFading = false
    private var fadeStart = 0L

    fun setKeys(height: Float, bar: Float, drawn: List<DrawnKey>) {
        boardHeight = height
        barHeight = bar
        keys = drawn
        emojiGrid.resize(width, height.toInt(), bar)
        requestLayout()
        invalidate()
    }

    fun setLook(pressedKey: Int, shift: ShiftLook) {
        pressed = pressedKey
        shiftLook = shift
        invalidate()
    }

    fun setPopup(command: Command.Popup) {
        popup = command.takeIf { it.items.isNotEmpty() }
        invalidate()
    }

    fun setForgetBubble(command: Command.ForgetBubble) {
        val label = context.getString(R.string.forget, command.word)
        popup = Command.Popup(command.x, 0f, command.width, 0, listOf(label))
        invalidate()
    }

    fun setUndo(offered: Boolean) {
        undoOffered = offered
        invalidate()
    }

    fun setSuggestions(command: Command.Suggestions) {
        suggestions = listOf(command.left, command.center, command.right)
        invalidate()
    }

    /** The emoji arrive after the panel opened, so what is on screen must be redrawn. */
    fun setEmoji(groups: List<Pair<String, List<EmojiItem>>>) {
        emojiGrid.setGroups(groups)
        invalidate()
    }

    fun setPanel(on: Boolean, recents: List<String>, tone: Int) {
        panelOpen = on
        if (on) emojiGrid.open(recents, tone)
        invalidate()
    }

    fun setTone(tone: Int) {
        emojiGrid.setTone(tone)
        invalidate()
    }

    fun setSearch(on: Boolean, query: String, results: List<String>) {
        if (searching != on) requestLayout()
        searching = on
        searchQuery = query
        searchResults = results
        invalidate()
    }

    fun setPreview(word: String) {
        preview = if (trailActive) word else ""
        invalidate()
    }

    fun setTrail(on: Boolean) {
        trailActive = on
        if (!on) preview = ""
        trailFading = !on
        if (!on) fadeStart = SystemClock.uptimeMillis()
        invalidate()
    }

    /** Keeps Gboard's distance to the screen edge in portrait, where height is plentiful. */
    private fun minimumBottom(): Int {
        val portrait = resources.configuration.orientation == Configuration.ORIENTATION_PORTRAIT
        return if (portrait) (MIN_BOTTOM_DP * density).toInt() else 0
    }

    /** The space kept free for the system's chevron, globe and gesture pill. */
    private fun reservedBottom(insets: WindowInsets): Int = maxOf(
        minimumBottom(),
        insets.getInsetsIgnoringVisibility(WindowInsets.Type.navigationBars()).bottom,
        insets.getInsetsIgnoringVisibility(WindowInsets.Type.tappableElement()).bottom,
        insets.getInsets(WindowInsets.Type.mandatorySystemGestures()).bottom,
    )

    override fun onApplyWindowInsets(insets: WindowInsets): WindowInsets {
        val reserved = reservedBottom(insets)
        if (reserved != bottomInset) {
            bottomInset = reserved
            requestLayout()
        }
        return super.onApplyWindowInsets(insets)
    }

    override fun onMeasure(widthSpec: Int, heightSpec: Int) {
        // The height depends on the width, and a layout request made during
        // layout is ignored, so the keys must be known before measuring ends.
        val measuredWidth = MeasureSpec.getSize(widthSpec)
        // The first measure can come before Android has any insets to give.
        bottomInset = rootWindowInsets?.let(::reservedBottom) ?: minimumBottom()
        if (measuredWidth != knownWidth) {
            knownWidth = measuredWidth
            send(Event.Resize(measuredWidth.toFloat(), resources.displayMetrics.heightPixels.toFloat(), density))
        }
        setMeasuredDimension(measuredWidth, boardHeight.toInt() + bottomInset + searchRow().toInt())
    }

    override fun onSizeChanged(width: Int, height: Int, oldWidth: Int, oldHeight: Int) {
        // Keep the system back gesture away from the edge keys (G-14).
        systemGestureExclusionRects = listOf(Rect(0, 0, width, height))
        emojiGrid.resize(width, boardHeight.toInt(), barHeight)
    }

    /** Extra height above the keys, so that opening the search never moves them. */
    private fun searchRow() = if (searching) barHeight else 0f

    override fun onDraw(canvas: Canvas) {
        guarded { drawBoard(canvas) }
    }

    private fun drawBoard(canvas: Canvas) {
        iconPaint.color = palette.text
        trailPaint.color = palette.accent
        canvas.drawColor(palette.background)
        if (panelOpen) {
            emojiGrid.draw(canvas)
            return
        }
        if (searching) drawSearchRow(canvas)
        canvas.translate(0f, searchRow())
        if (searching) drawSearch(canvas) else drawSuggestions(canvas)
        keys.forEachIndexed { index, key -> drawKey(canvas, index, key) }
        popup?.let { drawPopup(canvas, it) }
        drawTrail(canvas)
    }

    private fun drawSuggestions(canvas: Canvas) {
        text.textSize = SUGGESTION_SP * density
        text.color = palette.text
        val baseline = barHeight / 2 - (text.descent() + text.ascent()) / 2
        if (undoOffered) {
            canvas.drawText(context.getString(R.string.undo), width * CENTER, baseline, text)
            return
        }
        if (trailActive) {
            // The word forming under the finger takes the bar's centre.
            canvas.drawText(preview, width * CENTER, baseline, text)
            return
        }
        val slots = SLOT_CENTERS.map { width * it }
        val room = width * SLOT_SHARE
        suggestions.zip(slots).forEach { (word, x) ->
            // A whole phrasing narrows to its slot rather than run into the next one.
            text.textScaleX = 1f
            text.textScaleX = minOf(1f, room / text.measureText(word).coerceAtLeast(1f))
            canvas.drawText(word, x, baseline, text)
        }
        text.textScaleX = 1f
    }

    /** The light query input: magnifier, typed text, caret and clear cross. */
    private fun drawSearchRow(canvas: Canvas) {
        val center = barHeight / 2
        Icons.drawSearch(canvas, barHeight / 2, center, ICON_DP * density * SEARCH_ICON_SHARE, iconPaint)
        text.textAlign = Paint.Align.LEFT
        text.textSize = SUGGESTION_SP * density
        text.color = palette.text
        val shown = searchQuery.ifEmpty { "" }
        canvas.drawText(shown, barHeight, center - (text.descent() + text.ascent()) / 2, text)
        val caret = barHeight + text.measureText(shown) + density
        iconPaint.style = Paint.Style.STROKE
        canvas.drawLine(caret, center - SUGGESTION_SP * density * CARET_REACH, caret, center + SUGGESTION_SP * density * CARET_REACH, iconPaint)
        text.textAlign = Paint.Align.CENTER
        val cross = ICON_DP * density * CLEAR_CROSS_SHARE
        val x = width - barHeight / 2
        canvas.drawLine(x - cross, center - cross, x + cross, center + cross, iconPaint)
        canvas.drawLine(x + cross, center - cross, x - cross, center + cross, iconPaint)
    }

    private fun drawSearch(canvas: Canvas) {
        val cell = width / SEARCH_CELLS.toFloat()
        text.color = palette.text
        text.textSize = EMOJI_RESULT_SP * density
        val baseline = barHeight / 2 - (text.descent() + text.ascent()) / 2
        searchResults.take(SEARCH_CELLS).forEachIndexed { index, emoji ->
            canvas.drawText(emoji, cell * index + cell / 2, baseline, text)
        }
    }

    private fun drawKey(canvas: Canvas, index: Int, key: DrawnKey) {
        val gapX = KEY_GAP_X_DP * density
        val gapY = KEY_GAP_Y_DP * density
        rect.set(key.x + gapX, key.y + gapY, key.x + key.width - gapX, key.y + key.height - gapY)
        fill.color = when {
            index == pressed -> palette.pressed
            key.isFunction -> palette.function
            else -> palette.key
        }
        val radius = if (key.isPill) rect.height() / 2 else KEY_RADIUS_DP * density
        canvas.drawRoundRect(rect, radius, radius, fill)
        if (key.icon != Icon.NONE) {
            val solid = key.icon == Icon.SHIFT && shiftLook != ShiftLook.OFF
            Icons.draw(canvas, key.icon, rect.centerX(), rect.centerY(), ICON_DP * density, iconPaint, solid)
            if (key.icon == Icon.SHIFT && shiftLook == ShiftLook.LOCKED) {
                val half = ICON_DP * density * LOCK_BAR_SHARE
                val y = rect.centerY() + ICON_DP * density * LOCK_BAR_DROP
                iconPaint.style = Paint.Style.STROKE
                canvas.drawLine(rect.centerX() - half, y, rect.centerX() + half, y, iconPaint)
            }
            return
        }
        val size = (if (key.isFunction) FUNCTION_SP else KEY_SP) * density
        text.textSize = minOf(size, rect.height() * MAX_TEXT_SHARE)
        text.color = palette.text
        val baseline = rect.centerY() - (text.descent() + text.ascent()) / 2
        canvas.drawText(key.label, rect.centerX(), baseline, text)
        if (key.hint.isNotEmpty()) {
            text.textSize = HINT_SP * density
            val inset = HINT_INSET_DP * density
            canvas.drawText(key.hint, rect.right - inset, rect.top + inset - text.ascent() * 0.5f, text)
        }
    }

    private fun drawPopup(canvas: Canvas, shown: Command.Popup) {
        val height = POPUP_HEIGHT_DP * density
        rect.set(shown.x, shown.y, shown.x + shown.itemWidth * shown.items.size, shown.y + height)
        fill.color = palette.function
        canvas.drawRoundRect(rect, KEY_RADIUS_DP * density, KEY_RADIUS_DP * density, fill)
        text.textSize = KEY_SP * density
        text.color = palette.text
        shown.items.forEachIndexed { index, item ->
            val left = shown.x + index * shown.itemWidth
            if (index == shown.selected) {
                rect.set(left, shown.y, left + shown.itemWidth, shown.y + height)
                fill.color = palette.pressed
                canvas.drawRoundRect(rect, KEY_RADIUS_DP * density, KEY_RADIUS_DP * density, fill)
            }
            val baseline = shown.y + height / 2 - (text.descent() + text.ascent()) / 2
            canvas.drawText(item, left + shown.itemWidth / 2, baseline, text)
        }
    }

    private fun drawTrail(canvas: Canvas) {
        if (!showTrail || trail.isEmpty || !(trailActive || trailFading)) return
        if (trailActive) {
            trailPaint.alpha = TRAIL_ALPHA
        } else {
            val age = SystemClock.uptimeMillis() - fadeStart
            if (age >= TRAIL_FADE_MS) {
                trail.reset()
                trailFading = false
                return
            }
            trailPaint.alpha = (TRAIL_ALPHA * (1f - age.toFloat() / TRAIL_FADE_MS)).toInt()
            // Timer justified: a fade has no event to end on but the clock of the frames.
            postInvalidateOnAnimation()
        }
        canvas.drawPath(trail, trailPaint)
    }

    /** Every point is kept from the first touch, so the trail starts on the first letter. */
    private fun addTrailPoint(x: Float, y: Float) {
        if (trail.isEmpty) trail.moveTo(x, y) else trail.lineTo(x, y)
    }

    /** The system's dark mode or colours can change while the keyboard is loaded. */
    override fun onConfigurationChanged(configuration: Configuration) {
        super.onConfigurationChanged(configuration)
        palette = Palette(context)
        invalidate()
    }

    /**
     * Touch exploration: TalkBack reports hovering, the core names the key under
     * the finger, and lifting types it.
     */
    override fun onHoverEvent(event: MotionEvent): Boolean {
        if (panelOpen) return false
        guarded {
            val y = event.y - searchRow()
            when (event.actionMasked) {
                MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_MOVE -> send(Event.Hover(event.x, y))
                MotionEvent.ACTION_HOVER_EXIT -> send(Event.HoverEnd(event.x, y, event.eventTime.toULong()))
            }
        }
        return true
    }

    override fun performClick(): Boolean = super.performClick()

    override fun onTouchEvent(event: MotionEvent): Boolean {
        guarded {
            if (panelOpen) emojiGrid.onTouch(event) else touched(event)
        }
        return true
    }

    private fun touched(event: MotionEvent) {
        val index = event.actionIndex
        val id = event.getPointerId(index)
        val t = event.eventTime.toULong()
        val shift = searchRow()
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                if (!trailActive) {
                    trail.reset()
                    trailFading = false
                    addTrailPoint(event.getX(index), event.getY(index) - shift)
                }
                send(Event.Down(id, event.getX(index), event.getY(index) - shift, t))
            }
            MotionEvent.ACTION_MOVE -> {
                for (pointer in 0 until event.pointerCount) {
                    val pointerId = event.getPointerId(pointer)
                    for (h in 0 until event.historySize) {
                        val x = event.getHistoricalX(pointer, h)
                        val y = event.getHistoricalY(pointer, h) - shift
                        send(Event.Move(pointerId, x, y, event.getHistoricalEventTime(h).toULong()))
                        addTrailPoint(x, y)
                    }
                    send(Event.Move(pointerId, event.getX(pointer), event.getY(pointer) - shift, t))
                    addTrailPoint(event.getX(pointer), event.getY(pointer) - shift)
                }
                if (trailActive) {
                    onGestureMoved()
                    invalidate()
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                send(Event.Up(id, event.getX(index), event.getY(index) - shift, t))
                if (event.actionMasked == MotionEvent.ACTION_UP) performClick()
            }
            MotionEvent.ACTION_CANCEL -> send(Event.Cancel)
        }
    }

    private companion object {
        const val SEARCH_ICON_SHARE = 0.8f
        const val EMOJI_RESULT_SP = 24f

        /** Sizes and gaps measured on a Gboard capture from a Pixel 9a, for thumb memory. */
        const val ICON_DP = 24f
        const val ICON_STROKE_DP = 2.2f
        const val KEY_GAP_X_DP = 2.75f
        const val KEY_GAP_Y_DP = 6.25f
        const val KEY_SP = 25f

        /** Clears the gesture bar in portrait, before Android reports its insets. */
        const val MIN_BOTTOM_DP = 61f

        /** Text never taller than this share of a key, on short landscape keys. */
        const val MAX_TEXT_SHARE = 0.56f
        const val HINT_SP = 11f
        const val HINT_INSET_DP = 6f
        const val FUNCTION_SP = 16f
        const val SUGGESTION_SP = 16f

        /** A slot's width, leaving a gap: three slots and a little air. */
        const val SLOT_SHARE = 0.3f
        const val CENTER = 0.5f

        /** How far the search caret reaches from the middle, in text heights. */
        const val CARET_REACH = 0.6f

        /** Half the clear cross, and the caps lock bar, as shares of an icon. */
        const val CLEAR_CROSS_SHARE = 0.25f
        const val LOCK_BAR_SHARE = 0.33f
        const val LOCK_BAR_DROP = 0.62f
        const val TRAIL_DP = 8f

        /** See-through enough to read the keys under the trail. */
        const val TRAIL_ALPHA = 216
        const val TRAIL_FADE_MS = 300L
    }
}
