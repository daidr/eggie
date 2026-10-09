# Theme gallery design QA

**Source visual truth:** `/var/folders/22/f701zzws39x66b6xwcynvf5w0000gn/T/codex-clipboard-0b98a739-0001-40a2-b0ec-2b6e823feb3d.png` (1852 × 976 pixels; source display density unknown).

**Implementation:** `/tmp/eggie-theme-gallery-final-after-scroll-fix-dark.png` (3456 × 2234 physical pixels, 1728 × 1117 macOS points at 2×). Eggie Settings initially opens at 920 × 740 points. The focused comparison is `/tmp/eggie-theme-gallery-final-design-compare.png`; both content crops were normalized to 900 pixels wide. State: dark interface with both light and dark theme galleries visible. Theme names and colors come from Eggie's installed catalog, so they differ from the reference catalog.

**Findings:** No remaining P0, P1, or P2 mismatch in the theme selector. The full-view comparison shows the same two-section hierarchy, horizontal preview strips, selected card rings, theme labels, terminal sample and overlapping palette dots. The focused card comparison shows matching rounded-rectangle proportions and continuous outer corners. Native settings chrome and the number of visible cards differ with the app window width. The selected theme colors reflect the user's actual themes.

**Fidelity check:** Typography uses the app's UI font and retains the reference's hierarchy. Card aspect ratio, internal spacing, section rhythm, and divider placement follow the reference. Preview colors and cursor samples use real theme values. There are no source image assets to reproduce; the cards are live UI previews. Labels use localized settings text and catalog theme names.

**Interaction check:** Search filtered “3024 Day”; hovering a card previewed its colors and leaving the strip restored the saved theme. A wheel event over the light gallery moved the cards horizontally while the section's vertical position stayed fixed; a wheel event outside the gallery moved the outer settings panel vertically. Light and dark appearance were captured; the user's “follow system” setting was restored afterward.

**Comparison history:** The first window capture clipped dark theme labels behind the fixed terminal preview. Increasing the initial Settings height to 740 points made both card rows and labels visible. The final capture above shows the corrected layout. After the scroll propagation report, the gallery consumed wheel events so the outer panel no longer moves with the card strip; the final dark capture and wheel interaction check verify that fix.

**Implementation checklist:** Complete. No follow-up visual fixes identified.

final result: passed
