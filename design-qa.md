# Theme gallery design QA

**Source visual truth:** `/var/folders/22/f701zzws39x66b6xwcynvf5w0000gn/T/codex-clipboard-0b98a739-0001-40a2-b0ec-2b6e823feb3d.png` (1852 × 976 pixels; source display density unknown).

**Implementation:** `/tmp/eggie-theme-dots-ring-v2-dark.png` (3456 × 2234 physical pixels, 1728 × 1117 macOS points at 2×). Eggie Settings initially opens at 920 × 740 points. The focused comparison is `/tmp/eggie-theme-dots-ring-v2-compare.png`; both content crops were normalized to 900 pixels wide. State: dark interface with both light and dark theme galleries visible. Theme names and colors come from Eggie's installed catalog, so they differ from the reference catalog.

**Findings:** No remaining P0, P1, or P2 mismatch in the theme selector. The full-view comparison shows the same two-section hierarchy, horizontal preview strips, selected card rings, theme labels, terminal sample and eight overlapping palette dots. The focused card comparison shows larger continuous corners, an outer selection ring separated from the card, and the original fine card border retained inside it. Native settings chrome and the number of visible cards differ with the app window width. Eggie keeps its built-in default before the divider, followed by the catalog's stable order; the selected card stays at its catalog position.

**Fidelity check:** Typography uses the app's UI font and retains the reference's hierarchy. Card aspect ratio, internal spacing, section rhythm, and divider placement follow the reference. Preview colors and cursor samples use real theme values. There are no source image assets to reproduce; the cards are live UI previews. Labels use localized settings text and catalog theme names.

**Interaction check:** Search filtered “3024 Day”; hovering a card previewed its colors and leaving the strip restored the saved theme. Selecting “Adwaita” and then “3024 Day” moved only the selection ring; “Builtin Light”, “3024 Day”, and “Adwaita” stayed in that order. A wheel event over the light gallery moved the cards horizontally while the section's vertical position stayed fixed; a wheel event outside the gallery moved the outer settings panel vertically. Light and dark appearance were captured; the user's “follow system” setting was restored afterward.

**Comparison history:** The first window capture clipped dark theme labels behind the fixed terminal preview. Increasing the initial Settings height to 740 points made both card rows and labels visible. After the scroll propagation report, the gallery consumed wheel events so the outer panel no longer moves with the card strip. User feedback then identified undersized corners and the wrong card before the divider. Increasing the outer radius from 21 to 34 points, enlarging the inner sample rounding, and pinning the built-in default without sorting by selection fixed those issues. The latest feedback identified small palette dots and a selection ring that replaced the original border. The final comparison above shows eight 21-point dots and a separate 3-point outer ring with a 3-point gap before the retained fine border.

**Implementation checklist:** Complete. No follow-up visual fixes identified.

final result: passed
