package se.synthe.synthboard

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.drawable.Icon
import android.os.Bundle
import android.util.Size
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InlineSuggestion
import android.view.inputmethod.InlineSuggestionsRequest
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.inline.InlinePresentationSpec
import androidx.autofill.inline.UiVersions
import androidx.autofill.inline.common.TextViewStyle
import androidx.autofill.inline.common.ViewStyle
import androidx.autofill.inline.v1.InlineSuggestionUi

/**
 * Shows a password manager's suggestions over the bar. Each one is a surface
 * drawn by the manager itself, so the keyboard never sees what it holds.
 */
@SuppressLint("ViewConstructor")
class InlineStrip(context: Context) : HorizontalScrollView(context) {
    private val density = resources.displayMetrics.density
    private val row = LinearLayout(context).apply { gravity = Gravity.CENTER }
    private var generation = 0
    private var covered = false

    init {
        visibility = GONE
        filterTouchesWhenObscured = true
        isHorizontalScrollBarEnabled = false
        isFillViewport = true
        setBackgroundColor(Palette(context).background)
        addView(row, LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.MATCH_PARENT))
    }

    /** Hides the strip under an open panel, without losing its suggestions. */
    fun setCovered(on: Boolean) {
        covered = on
        refresh()
    }

    fun setHeight(height: Int) {
        layoutParams?.let {
            it.height = height
            layoutParams = it
        }
    }

    /** Replaces the suggestions once all of them are drawn, in the given order. */
    fun inflate(suggestions: List<InlineSuggestion>) {
        val current = ++generation
        val views = arrayOfNulls<View>(suggestions.size)
        var waiting = suggestions.size
        if (waiting == 0) show(emptyList())
        val anySize = Size(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT)
        suggestions.forEachIndexed { index, suggestion ->
            suggestion.inflate(context, anySize, context.mainExecutor) { view ->
                views[index] = view
                waiting -= 1
                if (waiting == 0 && current == generation) show(views.filterNotNull())
            }
        }
    }

    fun clear() {
        generation += 1
        show(emptyList())
    }

    fun show(views: List<View>) {
        row.removeAllViews()
        views.forEach { view ->
            // Android sizes each suggestion to its surface, which cannot wrap its content.
            val params = view.layoutParams?.let(LinearLayout::LayoutParams)
                ?: LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT)
            params.marginStart = (CHIP_GAP_DP * density / 2).toInt()
            params.marginEnd = params.marginStart
            row.addView(view, params)
        }
        refresh()
    }

    private fun refresh() {
        visibility = if (row.childCount > 0 && !covered) VISIBLE else GONE
    }

    companion object {
        private const val MAX_SUGGESTIONS = 6
        private const val CHIP_GAP_DP = 8f
        private const val CHIP_ACROSS_PADDING_DP = 12f
        private const val CHIP_ALONG_PADDING_DP = 8f
        private const val MIN_CHIP_WIDTH_DP = 48f
        private const val MAX_CHIP_HEIGHT_DP = 40f
        private const val TITLE_SP = 14f
        private const val SUBTITLE_SP = 12f

        /** Tells Android how the bar can show suggestions. */
        fun request(context: Context): InlineSuggestionsRequest {
            val density = context.resources.displayMetrics.density
            val spec = InlinePresentationSpec.Builder(
                Size((MIN_CHIP_WIDTH_DP * density).toInt(), 0),
                Size(context.resources.displayMetrics.widthPixels, (MAX_CHIP_HEIGHT_DP * density).toInt()),
            ).setStyle(style(context)).build()
            return InlineSuggestionsRequest.Builder(listOf(spec)).setMaxSuggestionCount(MAX_SUGGESTIONS).build()
        }

        private fun style(context: Context): Bundle {
            val palette = Palette(context)
            val density = context.resources.displayMetrics.density
            val across = (CHIP_ACROSS_PADDING_DP * density).toInt()
            val along = (CHIP_ALONG_PADDING_DP * density).toInt()
            val chip = ViewStyle.Builder()
                .setBackground(Icon.createWithResource(context, R.drawable.inline_chip).setTint(palette.key))
                .setPadding(across, along, across, along)
                .build()
            val title = TextViewStyle.Builder().setTextColor(palette.text).setTextSize(TITLE_SP).build()
            val subtitle = TextViewStyle.Builder().setTextColor(palette.text).setTextSize(SUBTITLE_SP).build()
            val ui = InlineSuggestionUi.newStyleBuilder()
                .setChipStyle(chip)
                .setTitleStyle(title)
                .setSubtitleStyle(subtitle)
                .build()
            return UiVersions.newStylesBuilder().addStyle(ui).build()
        }
    }
}
