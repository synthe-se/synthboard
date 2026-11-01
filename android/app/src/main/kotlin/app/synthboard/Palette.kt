package app.synthboard

import android.content.Context
import android.content.res.Configuration

/** The keyboard's colours, from the system's theme and dynamic colours. */
class Palette(context: Context) {
    private val dark = context.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
        Configuration.UI_MODE_NIGHT_YES

    private val pick = { night: Int, day: Int -> context.getColor(if (dark) night else day) }

    val background = pick(android.R.color.system_neutral1_900, android.R.color.system_neutral2_100)
    val key = pick(android.R.color.system_neutral2_800, android.R.color.system_neutral1_0)
    val function = pick(android.R.color.system_neutral2_700, android.R.color.system_neutral2_200)
    val pressed = pick(android.R.color.system_accent1_600, android.R.color.system_accent1_200)
    val accent = pick(android.R.color.system_accent1_300, android.R.color.system_accent1_600)
    val text = pick(android.R.color.system_neutral1_50, android.R.color.system_neutral1_900)
}
