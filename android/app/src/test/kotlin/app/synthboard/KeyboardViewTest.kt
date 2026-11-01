package app.synthboard

import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.SystemClock
import android.view.MotionEvent
import android.view.View
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.synthboard.core.Command
import app.synthboard.core.EmojiItem
import app.synthboard.core.Event
import app.synthboard.core.ShiftLook
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

private const val PORTRAIT = "w411dp-h923dp-port-xxhdpi"
private const val LANDSCAPE = "w923dp-h411dp-land-xxhdpi"

/** Tests the shell alone: it must draw what it is told and report what it sees. */
@RunWith(AndroidJUnit4::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = PORTRAIT)
class KeyboardViewTest {
    private val sent = mutableListOf<Event>()
    private val context = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val density get() = context.resources.displayMetrics.density

    private fun view(): KeyboardView {
        val width = context.resources.displayMetrics.widthPixels
        val screen = context.resources.displayMetrics.heightPixels
        val bar = 48f * density
        val row = minOf(62.4f * density, (screen * 0.5f - bar) / 4f)
        return KeyboardView(context) { sent += it }.apply {
            setKeys(bar + row * 4f, bar, ShellFixtures.keys(width.toFloat(), row, bar))
            layOut(width)
        }
    }

    @Test
    fun emojiThatArriveAfterThePanelOpenedAreDrawn() {
        val keyboard = view()
        keyboard.setPanel(true, listOf("👍"), 0)
        val shadow = org.robolectric.Shadows.shadowOf(keyboard)
        shadow.clearWasInvalidated()
        keyboard.setEmoji(listOf("Smileys" to listOf(EmojiItem("😀", emptyList()))))
        assertTrue(shadow.wasInvalidated())
    }

    private fun View.layOut(width: Int) {
        measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(0, View.MeasureSpec.UNSPECIFIED),
        )
        layout(0, 0, measuredWidth, measuredHeight)
    }

    /** Draws the view as the window would, since an input view has no activity. */
    private fun View.snapshot(path: String) {
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        draw(Canvas(bitmap))
        bitmap.captureRoboImage(path)
    }

    private fun touch(action: Int, x: Float, y: Float): MotionEvent {
        val now = SystemClock.uptimeMillis()
        return MotionEvent.obtain(now, now, action, x, y, 0)
    }

    @Test
    fun portrait_keyboard() = view().snapshot("src/test/snapshots/portrait.png")

    @Config(qualifiers = "w411dp-h923dp-port-night-xxhdpi")
    @Test
    fun dark_keyboard() = view().snapshot("src/test/snapshots/dark.png")

    @Config(qualifiers = LANDSCAPE)
    @Test
    fun landscape_keyboard() = view().snapshot("src/test/snapshots/landscape.png")

    @Test
    fun suggestions_and_shift() {
        val keyboard = view()
        keyboard.setSuggestions(Command.Suggestions("bonjour", "bonsoir", "bonheur"))
        keyboard.setLook(-1, ShiftLook.LOCKED)
        keyboard.snapshot("src/test/snapshots/suggestions.png")
    }

    @Test
    fun undo_bar() {
        val keyboard = view()
        keyboard.setUndo(true)
        keyboard.snapshot("src/test/snapshots/undo-bar.png")
    }

    @Test
    fun accent_popup() {
        val keyboard = view()
        keyboard.setPopup(Command.Popup(100f, 60f, 115f, 1, listOf("é", "è", "ê", "ë", "€")))
        keyboard.snapshot("src/test/snapshots/popup.png")
    }

    @Test
    fun emoji_panel() {
        val keyboard = view()
        keyboard.setEmoji(
            listOf("Smileys" to "😀😃😄😁😅😂🤣😊😇🙂😉😌".chunked(2).map { EmojiItem(it, emptyList()) }),
        )
        keyboard.setPanel(true, listOf("👍"), 0)
        keyboard.layOut(context.resources.displayMetrics.widthPixels)
        keyboard.snapshot("src/test/snapshots/emoji.png")
    }

    @Test
    fun emoji_search_row() {
        val keyboard = view()
        keyboard.setSearch(true, "pizz", listOf("🍕"))
        keyboard.layOut(context.resources.displayMetrics.widthPixels)
        keyboard.snapshot("src/test/snapshots/search.png")
    }

    @Test
    fun portrait_keeps_the_room_gboard_keeps_under_the_keys() {
        val keyboard = view()
        val keys = (48f + 62.4f * 4f) * density
        assertTrue(keyboard.measuredHeight >= keys + 61f * density - 1f)
    }

    @Config(qualifiers = LANDSCAPE)
    @Test
    fun landscape_leaves_the_upper_half_of_the_screen_free() {
        val screen = context.resources.displayMetrics.heightPixels
        assertTrue(view().measuredHeight <= screen * 0.6f)
    }

    @Test
    fun the_screen_is_reported_once_for_a_width() {
        val keyboard = view()
        keyboard.layOut(context.resources.displayMetrics.widthPixels)
        assertEquals(1, sent.count { it is Event.Resize })
        val resize = sent.filterIsInstance<Event.Resize>().single()
        assertEquals(context.resources.displayMetrics.heightPixels.toFloat(), resize.screenHeight)
    }

    @Test
    fun touches_are_reported_with_their_position() {
        val keyboard = view()
        sent.clear()
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_DOWN, 100f, 400f))
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_UP, 110f, 410f))
        val down = sent[0] as Event.Down
        val up = sent[1] as Event.Up
        assertEquals(100f, down.x)
        assertEquals(400f, down.y)
        assertEquals(110f, up.x)
    }

    @Test
    fun the_search_row_shifts_touches_down_by_its_height() {
        val keyboard = view()
        keyboard.setSearch(true, "", emptyList())
        keyboard.layOut(context.resources.displayMetrics.widthPixels)
        sent.clear()
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_DOWN, 100f, 400f))
        assertEquals(400f - 48f * density, (sent[0] as Event.Down).y, 0.5f)
    }

    @Test
    fun a_cancelled_touch_is_reported() {
        val keyboard = view()
        sent.clear()
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_CANCEL, 0f, 0f))
        assertTrue(sent.single() is Event.Cancel)
    }

    @Test
    fun a_moving_finger_during_a_gesture_asks_for_a_preview() {
        val keyboard = view()
        var previews = 0
        keyboard.onGestureMoved = { previews += 1 }
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_MOVE, 10f, 400f))
        assertEquals(0, previews)
        keyboard.setTrail(true)
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_MOVE, 20f, 400f))
        assertEquals(1, previews)
    }

    @Test
    fun a_late_preview_after_the_gesture_shows_nothing() {
        val keyboard = view()
        keyboard.setTrail(true)
        keyboard.setTrail(false)
        keyboard.setPreview("bonjour")
        keyboard.snapshot("src/test/snapshots/late-preview.png")
    }

    @Test
    fun touch_exploration_hovers_then_types_on_exit() {
        val keyboard = view()
        sent.clear()
        keyboard.onHoverEvent(touch(MotionEvent.ACTION_HOVER_MOVE, 50f, 400f))
        keyboard.onHoverEvent(touch(MotionEvent.ACTION_HOVER_EXIT, 50f, 400f))
        assertTrue(sent[0] is Event.Hover)
        assertTrue(sent[1] is Event.HoverEnd)
    }

    @Test
    fun the_panel_keeps_touches_away_from_the_keys() {
        val keyboard = view()
        keyboard.setPanel(true, emptyList(), 0)
        sent.clear()
        keyboard.onTouchEvent(touch(MotionEvent.ACTION_DOWN, 100f, 400f))
        assertTrue(sent.none { it is Event.Down })
    }
}
