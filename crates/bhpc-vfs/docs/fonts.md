# System fonts (CoreGraphics / Apportable)

## What the game does

`libCoreGraphics.so` (Apportable's CG on top of Skia/fontconfig and libxml2) builds the font database in `_CGFontDBCreateSystem`.
From the strings of the 1.7.4 library:

- Config files, parsed with `xmlReaderForFile` / `xmlTextReader*` (only the text of `<name>` and `<file>` is compared):
  `/system/etc/system_fonts.xml`, `/system/etc/fallback_fonts.xml`, `/vendor/etc/fallback_fonts.xml`.
- Font files: `/system/fonts/%s` for each `<file>`.
- Log lines: `CGFONTDB: Started parsing system fonts xml file %s`, `CGFONTDB: SYSTEM_FONTS path is not found! Using fallback
  hardcoded system fonts`, `CGFONTDB: FALLBACK_FONTS or VENDOR_FONTS path are not found!`, `CGFONTDB Unable to open %s`.
- Hard-coded fallback list (used when the XML is missing, which is the log line seen on current Android): `/system/fonts/`
  `DroidSans.ttf`, `DroidSans-Bold.ttf`, `DroidSansArabic.ttf`, `DroidSansFallback.ttf`, `DroidSansHebrew.ttf`,
  `DroidSansMono.ttf`, `DroidSansThai.ttf`, `DroidSerif-Regular.ttf`, `-Bold`, `-Italic`, `-BoldItalic`.
- Family names it knows: `DroidSans`, `DroidSans-Bold`, `DroidSans-Italic`, `Roboto-Regular`, `Roboto-Bold`, `Roboto-Italic`,
  `RobotoCondensed-Bold`. `libCoreText.so` asks for `Roboto-Regular`. `libGraphicsServices.so` (fontconfig) scans `/system/fonts`.
- `libApplication.so` references `/system/fonts` directly. The game's own UI text uses its bitmap fonts and `BlockheadsFont-Regular.otf`
  from `assets/`, so system fonts matter for UIKit text (alerts, IAP store UI, text fields, non-Latin scripts).

The APK ships `com.apportable.system/fonts/` with `DroidNaskh-Regular-SystemUI.ttf`, `DroidSansHebrew-{Regular,Bold}.ttf` and
`DroidSansThai.ttf` (Android system fonts the user already owns inside their APK; read at runtime, never copied out).

On devices from Android 5 on, `/system/etc/system_fonts.xml` and the `Droid*` files are normally absent (replaced by `fonts.xml`
and Roboto/Noto). That is the `SYSTEM_FONTS path is not found` case, and the hard-coded files are missing too, which is our working
theory for the 1.7.6 IAP UI crash that `fix-iap-store-font-crash` works around (not confirmed in a debugger). On the PC every name
resolves, so the patch should not be needed.

## What `bhpc-vfs` serves

`/system/etc/system_fonts.xml` is generated in the Android 4.4 format with four families (`sans-serif` and aliases -> Roboto,
`sans-serif-condensed`, `serif` -> DroidSerif, `monospace` -> DroidSansMono), `fallback_fonts.xml` lists the Arabic, Hebrew, Thai
and Fallback fonts, and `/vendor/etc/fallback_fonts.xml` is an empty `<familyset>`. Every file they (and the hard-coded list) name
exists under `/system/fonts/`, resolved in this order:

1. `<data root>/fonts/<name>`: a user override (drop in any TTF/OTF under the expected name, for example a CJK font as
   `DroidSansFallback.ttf`).
2. The APK's `com.apportable.system/fonts/<name>`, with aliases `DroidSansHebrew.ttf` -> `DroidSansHebrew-Regular.ttf` and
   `DroidSansArabic.ttf` -> `DroidNaskh-Regular-SystemUI.ttf`.
3. A bundled SIL OFL 1.1 font: **Liberation Sans** 2.1.5 Regular / Bold / Italic (`crates/bhpc-vfs/fonts/`, license in
   `LICENSE-OFL.txt`), chosen by the style in the file name (`*Bold*` -> Bold, `*Italic*` -> Italic, bold italic -> Bold, else
   Regular). Roboto, Droid Serif and Droid Sans Mono requests therefore render in Liberation Sans. No Apple or Google proprietary font
   is included.

## Limits and open points

- Liberation Sans has Latin/Greek/Cyrillic only. CJK and Devanagari fall back to whatever `DroidSansFallback.ttf` is (the Latin font
  by default), so those glyphs are missing unless the user adds an override.
- Metrics differ from Roboto/Droid Sans slightly, so text widths in alerts and the IAP UI shift a few pixels.
- The XML dialect was derived from the library's strings and the Android 4.4 format; it has not been run against the guest parser yet
  (needs libxml2 and CoreGraphics running, M4+). If `CGFONTDB ... failed to parse` appears in the log, adjust `fonts::system_fonts_xml`.
