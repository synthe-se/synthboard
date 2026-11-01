package app.synthboard

import app.synthboard.core.DrawnKey
import app.synthboard.core.Icon

/** A keyboard as the core would describe it, for testing the shell alone. */
object ShellFixtures {
    private val rows = listOf("azertyuiop", "qsdfghjklm")

    fun keys(width: Float, rowHeight: Float, barHeight: Float): List<DrawnKey> {
        val cell = width / 10f
        fun letter(label: String, column: Float, row: Int, span: Float = 1f, hint: String = "") = DrawnKey(
            column * cell,
            barHeight + row * rowHeight,
            span * cell,
            rowHeight,
            label,
            hint,
            Icon.NONE,
            false,
            false,
        )
        fun function(icon: Icon, column: Float, row: Int, span: Float, label: String = "", pill: Boolean = false) = DrawnKey(
            column * cell,
            barHeight + row * rowHeight,
            span * cell,
            rowHeight,
            label,
            "",
            icon,
            true,
            pill,
        )
        return buildList {
            rows.forEachIndexed { row, letters ->
                letters.forEachIndexed { column, key ->
                    val digit = if (row == 0) "1234567890"[column].toString() else ""
                    add(letter(key.toString(), column.toFloat(), row, hint = digit))
                }
            }
            add(function(Icon.SHIFT, 0f, 2, 1.5f))
            "wxcvbn'".forEachIndexed { column, key -> add(letter(key.toString(), 1.5f + column, 2)) }
            add(function(Icon.DELETE, 8.5f, 2, 1.5f))
            add(function(Icon.NONE, 0f, 3, 1.5f, "?123", pill = true))
            add(letter(",", 1.5f, 3))
            add(function(Icon.EMOJI, 2.5f, 3, 1f))
            add(letter("", 3.5f, 3, 4f))
            add(letter(".", 7.5f, 3))
            add(function(Icon.ENTER, 8.5f, 3, 1.5f, pill = true))
        }
    }
}
