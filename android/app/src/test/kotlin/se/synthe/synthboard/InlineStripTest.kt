package se.synthe.synthboard

import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.autofill.inline.UiVersions
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.annotation.Config

@RunWith(AndroidJUnit4::class)
@Config(sdk = [35])
class InlineStripTest {
    private val context = ApplicationProvider.getApplicationContext<android.content.Context>()

    private fun chip() = TextView(context).apply { text = "account" }

    @Test
    fun the_strip_appears_only_with_suggestions() {
        val strip = InlineStrip(context)
        assertEquals(View.GONE, strip.visibility)
        strip.show(listOf(chip()))
        assertEquals(View.VISIBLE, strip.visibility)
        strip.clear()
        assertEquals(View.GONE, strip.visibility)
    }

    @Test
    fun a_suggestion_keeps_the_size_android_gave_it() {
        val strip = InlineStrip(context)
        val suggestion = chip().apply { layoutParams = ViewGroup.LayoutParams(512, 89) }
        strip.show(listOf(suggestion))
        assertEquals(512, suggestion.layoutParams.width)
        assertEquals(89, suggestion.layoutParams.height)
    }

    @Test
    fun a_lone_suggestion_sits_in_the_middle() {
        val strip = InlineStrip(context)
        val suggestion = chip().apply { layoutParams = ViewGroup.LayoutParams(200, 80) }
        strip.show(listOf(suggestion))
        strip.measure(
            View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(120, View.MeasureSpec.EXACTLY),
        )
        strip.layout(0, 0, 1000, 120)
        assertEquals(400, suggestion.left)
        assertEquals(20, suggestion.top)
    }

    @Test
    fun a_panel_hides_the_strip_and_gives_it_back() {
        val strip = InlineStrip(context)
        strip.show(listOf(chip()))
        strip.setCovered(true)
        assertEquals(View.GONE, strip.visibility)
        strip.setCovered(false)
        assertEquals(View.VISIBLE, strip.visibility)
    }

    @Test
    fun the_request_carries_a_style_managers_can_read() {
        val request = InlineStrip.request(context)
        assertEquals(1, request.inlinePresentationSpecs.size)
        val spec = request.inlinePresentationSpecs.first()
        assertTrue(spec.maxSize.height > 0)
        assertTrue(UiVersions.getVersions(spec.style).contains(UiVersions.INLINE_UI_VERSION_1))
    }
}
