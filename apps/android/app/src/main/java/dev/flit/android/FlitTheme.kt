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

/**
 * One face, the one the phone already draws: the system sans. Flit is a
 * utility that looks like it belongs to the device, so the hierarchy comes
 * from weight and size rather than a bundled display face. Everything —
 * wordmark, section titles, device names, sizes, fingerprints — is the same
 * family; only the weight changes.
 */
private val Sans = FontFamily.SansSerif

// Paper and ink. The accent is spent only on what can be acted on: the one
// button, the selection check, the progress fill. Hierarchy is carried by
// tone and type, never by a border around every box.
private val Paper = Color(0xFFF7F6F2)
private val Card = Color(0xFFFFFFFF)
private val Raised = Color(0xFFEFECE6)
private val Line = Color(0xFFE6E2DA)
private val Ink = Color(0xFF14161A)
private val Ink2 = Color(0xFF575E66)
private val Ink3 = Color(0xFF666E78)
private val Accent = Color(0xFF0B6B4F)
private val AccentSoft = Color(0xFFE6F2EC)
private val Danger = Color(0xFFB3261E)
private val DangerSoft = Color(0xFFFCEBE8)

private val NightGround = Color(0xFF14171A)
private val NightSurface = Color(0xFF1B1F23)
private val NightRaised = Color(0xFF24292F)
private val NightLine = Color(0xFF2A2F36)
private val NightInk = Color(0xFFEEF1F4)
private val NightInk2 = Color(0xFFA8B1BB)
private val NightInk3 = Color(0xFF79838D)
private val NightAccent = Color(0xFF3EC49A)
private val NightAccentSoft = Color(0xFF123A30)
private val NightDanger = Color(0xFFFF8A80)
private val NightDangerSoft = Color(0xFF3A1D1A)

/** Colours the screens need beyond Material's slots. */
data class FlitColors(
    val ink2: Color,
    val ink3: Color,
    val line: Color,
    val card: Color,
    val raised: Color,
    val paper: Color,
    val accentSoft: Color,
    val dangerSoft: Color,
)

private val LocalFlitColors = androidx.compose.runtime.staticCompositionLocalOf {
    FlitColors(
        ink2 = Ink2,
        ink3 = Ink3,
        line = Line,
        card = Card,
        raised = Raised,
        paper = Paper,
        accentSoft = AccentSoft,
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
            background = NightGround,
            onBackground = NightInk,
            surface = NightSurface,
            onSurface = NightInk,
            surfaceVariant = NightRaised,
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
            surface = Card,
            onSurface = Ink,
            surfaceVariant = Raised,
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
            ink3 = NightInk3,
            line = NightLine,
            card = NightSurface,
            raised = NightRaised,
            paper = NightGround,
            accentSoft = NightAccentSoft,
            dangerSoft = NightDangerSoft,
        )
    } else {
        FlitColors(
            ink2 = Ink2,
            ink3 = Ink3,
            line = Line,
            card = Card,
            raised = Raised,
            paper = Paper,
            accentSoft = AccentSoft,
            dangerSoft = DangerSoft,
        )
    }

    androidx.compose.runtime.CompositionLocalProvider(LocalFlitColors provides extra) {
        MaterialTheme(
            colorScheme = scheme,
            typography = Typography(
                displaySmall = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 32.sp,
                    lineHeight = 38.sp,
                    letterSpacing = (-0.6).sp,
                ),
                // Section titles and the state words of a transfer.
                headlineSmall = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 24.sp,
                    lineHeight = 30.sp,
                    letterSpacing = (-0.4).sp,
                ),
                // Device names, and the name in the identity sheet.
                titleLarge = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 20.sp,
                    lineHeight = 26.sp,
                    letterSpacing = (-0.2).sp,
                ),
                // Factual headlines: sans, small, medium weight.
                titleMedium = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 16.sp,
                    lineHeight = 21.sp,
                    letterSpacing = (-0.1).sp,
                ),
                titleSmall = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.Medium,
                    fontSize = 15.sp,
                    lineHeight = 20.sp,
                ),
                bodyLarge = TextStyle(
                    fontFamily = Sans,
                    fontSize = 16.sp,
                    lineHeight = 24.sp,
                ),
                bodyMedium = TextStyle(
                    fontFamily = Sans,
                    fontSize = 14.sp,
                    lineHeight = 21.sp,
                ),
                bodySmall = TextStyle(
                    fontFamily = Sans,
                    fontSize = 12.5.sp,
                    lineHeight = 18.sp,
                ),
                labelMedium = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.Medium,
                    fontSize = 13.5.sp,
                    lineHeight = 18.sp,
                ),
                labelSmall = TextStyle(
                    fontFamily = Sans,
                    fontWeight = FontWeight.Medium,
                    fontSize = 12.sp,
                    lineHeight = 16.sp,
                ),
            ),
            content = content,
        )
    }
}
