package app.synthboard

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import app.synthboard.core.Icon

/** One family of key icons: same grid, same stroke, drawn around a centre. */
object Icons {
    private const val GRID = 24f
    private val path = Path()

    /** Draws `icon` in a square of `size` pixels. `solid` fills the shape. */
    fun draw(canvas: Canvas, icon: Icon, cx: Float, cy: Float, size: Float, stroke: Paint, solid: Boolean = false) {
        canvas.save()
        canvas.translate(cx - size / 2, cy - size / 2)
        canvas.scale(size / GRID, size / GRID)
        val width = stroke.strokeWidth
        stroke.strokeWidth = width * GRID / size
        stroke.style = if (solid) Paint.Style.FILL_AND_STROKE else Paint.Style.STROKE
        path.reset()
        when (icon) {
            Icon.SHIFT -> {
                path.moveTo(12f, 4f)
                path.lineTo(20f, 13f)
                path.lineTo(15.5f, 13f)
                path.lineTo(15.5f, 20f)
                path.lineTo(8.5f, 20f)
                path.lineTo(8.5f, 13f)
                path.lineTo(4f, 13f)
                path.close()
                canvas.drawPath(path, stroke)
            }
            Icon.DELETE -> {
                path.moveTo(9f, 5f)
                path.lineTo(21f, 5f)
                path.lineTo(21f, 19f)
                path.lineTo(9f, 19f)
                path.lineTo(3f, 12f)
                path.close()
                path.moveTo(11.5f, 9f)
                path.lineTo(17.5f, 15f)
                path.moveTo(17.5f, 9f)
                path.lineTo(11.5f, 15f)
                canvas.drawPath(path, stroke)
            }
            Icon.ENTER -> {
                path.moveTo(20f, 5f)
                path.lineTo(20f, 14f)
                path.lineTo(5f, 14f)
                path.moveTo(9.5f, 9.5f)
                path.lineTo(5f, 14f)
                path.lineTo(9.5f, 18.5f)
                canvas.drawPath(path, stroke)
            }
            Icon.EMOJI -> {
                canvas.drawCircle(12f, 12f, 9f, stroke)
                path.moveTo(7.8f, 14f)
                path.quadTo(12f, 18.5f, 16.2f, 14f)
                path.moveTo(9f, 9.2f)
                path.lineTo(9f, 10.2f)
                path.moveTo(15f, 9.2f)
                path.lineTo(15f, 10.2f)
                canvas.drawPath(path, stroke)
            }
            Icon.NONE -> Unit
        }
        stroke.strokeWidth = width
        canvas.restore()
    }

    /** A magnifier for the emoji search row, on the same grid and stroke. */
    fun drawSearch(canvas: Canvas, cx: Float, cy: Float, size: Float, stroke: Paint) {
        canvas.save()
        canvas.translate(cx - size / 2, cy - size / 2)
        canvas.scale(size / GRID, size / GRID)
        val width = stroke.strokeWidth
        stroke.strokeWidth = width * GRID / size
        stroke.style = Paint.Style.STROKE
        canvas.drawCircle(10.5f, 10.5f, 6.5f, stroke)
        canvas.drawLine(15.5f, 15.5f, 20f, 20f, stroke)
        stroke.strokeWidth = width
        canvas.restore()
    }
}
