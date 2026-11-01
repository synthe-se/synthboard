package se.synthe.synthboard

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF

/** Where text sits so that it looks centred on `y`, whatever its font. */
internal fun Paint.centeredOn(y: Float): Float = y - (descent() + ascent()) / 2

/** A row of choices in a rounded bubble, the selected one lit: accents and skin tones. */
internal class Bubble(private val radius: Float) {
    private val rect = RectF()
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)

    fun draw(canvas: Canvas, area: RectF, items: List<String>, selected: Int, text: Paint, background: Int, lit: Int) {
        val itemWidth = area.width() / items.size
        fill.color = background
        canvas.drawRoundRect(area, radius, radius, fill)
        val baseline = text.centeredOn(area.centerY())
        items.forEachIndexed { index, item ->
            val left = area.left + index * itemWidth
            if (index == selected) {
                rect.set(left, area.top, left + itemWidth, area.bottom)
                fill.color = lit
                canvas.drawRoundRect(rect, radius, radius, fill)
            }
            canvas.drawText(item, left + itemWidth / 2, baseline, text)
        }
    }
}
