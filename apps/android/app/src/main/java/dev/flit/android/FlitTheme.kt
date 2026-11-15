package dev.flit.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

// The same paper-and-ink palette the desktop window uses, so the two screens
// look like one product. Nothing here is decorative: the accent is reserved for
// this device, the ink steps carry hierarchy, lines are borders you can see.
private val Paper = Color(0xFFF6F5F2)
private val Surface = Color(0xFFFFFFFF)
private val Surface2 = Color(0xFFFAF9F7)
private val Line = Color(0xFFE9E6E0)
private val Ink = Color(0xFF15181C)
private val Ink2 = Color(0xFF565E68)
private val Accent = Color(0xFF0E7A5C)
private val AccentSoft = Color(0xFFE9F5F0)
private val AccentLine = Color(0xFFA9D8C8)
private val Danger = Color(0xFFC0392F)
private val DangerSoft = Color(0xFFFDECEB)

private val NightPaper = Color(0xFF14171A)
private val NightSurface = Color(0xFF1B1F23)
private val NightSurface2 = Color(0xFF20242A)
private val NightLine = Color(0xFF2A2F36)
private val NightInk = Color(0xFFEEF1F4)
private val NightInk2 = Color(0xFFA8B1BB)
private val NightAccent = Color(0xFF3EC49A)
private val NightAccentSoft = Color(0xFF123A30)
private val NightAccentLine = Color(0xFF2C6D5C)
private val NightDanger = Color(0xFFFF8A80)
private val NightDangerSoft = Color(0xFF3A1D1A)

/** Extra colours the screen needs beyond Material's slots. */
data class FlitColors(
    val ink2: Color,
    val line: Color,
    val card: Color,
    val accentSoft: Color,
    val accentLine: Color,
    val dangerSoft: Color,
)

private val LocalFlitColors = androidx.compose.runtime.staticCompositionLocalOf {
    FlitColors(
        ink2 = Ink2,
        line = Line,
        card = Surface,
        accentSoft = AccentSoft,
        accentLine = AccentLine,
        dangerSoft = DangerSoft,
    )
}

object FlitPalette {
    val colors: FlitColors
        @Composable get() = LocalFlitColors.current
}

@Composable
fun FlitTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()

    val scheme = if (dark) {
        darkColorScheme(
            primary = NightAccent,
            onPrimary = Color(0xFF06231B),
            background = NightPaper,
            onBackground = NightInk,
            surface = NightSurface,
            onSurface = NightInk,
            surfaceVariant = NightSurface2,
            onSurfaceVariant = NightInk2,
            outline = NightLine,
            error = NightDanger,
            onError = Color(0xFF2B0F0C),
            errorContainer = NightDangerSoft,
        )
    } else {
        lightColorScheme(
            primary = Accent,
            onPrimary = Color.White,
            background = Paper,
            onBackground = Ink,
            surface = Surface,
            onSurface = Ink,
            surfaceVariant = Surface2,
            onSurfaceVariant = Ink2,
            outline = Line,
            error = Danger,
            onError = Color.White,
            errorContainer = DangerSoft,
        )
    }

    val extra = if (dark) {
        FlitColors(
            ink2 = NightInk2,
            line = NightLine,
            card = NightSurface,
            accentSoft = NightAccentSoft,
            accentLine = NightAccentLine,
            dangerSoft = NightDangerSoft,
        )
    } else {
        FlitColors(
            ink2 = Ink2,
            line = Line,
            card = Surface,
            accentSoft = AccentSoft,
            accentLine = AccentLine,
            dangerSoft = DangerSoft,
        )
    }

    androidx.compose.runtime.CompositionLocalProvider(LocalFlitColors provides extra) {
        MaterialTheme(
            colorScheme = scheme,
            typography = Typography(
                // Android's system face is deliberate: bundling a webfont into
                // the APK costs a megabyte to look like every other app.
                displaySmall = TextStyle(
                    fontFamily = FontFamily.SansSerif,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 30.sp,
                    letterSpacing = (-0.6).sp,
                ),
                titleMedium = TextStyle(
                    fontFamily = FontFamily.SansSerif,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 16.sp,
                ),
                bodyMedium = TextStyle(
                    fontFamily = FontFamily.SansSerif,
                    fontSize = 14.sp,
                ),
                labelSmall = TextStyle(
                    fontFamily = FontFamily.SansSerif,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 11.sp,
                    letterSpacing = 1.1.sp,
                ),
            ),
            content = content,
        )
    }
}
