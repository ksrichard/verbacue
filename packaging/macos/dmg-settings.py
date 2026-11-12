# dmgbuild settings for the verbacue DMG. Used by make-dmg.sh:
#   dmgbuild -s dmg-settings.py -D app=<verbacue.app> verbacue <out.dmg>
# The window shows the app, a link to /Applications and the background from
# make-background.py, whose layout these positions must match.
import os

here = os.path.dirname(os.path.abspath(settings_file))  # noqa: F821
app = defines["app"]  # noqa: F821

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(here, "..", "icons", "verbacue.icns")
background = os.path.join(here, "dmg-background.png")  # @2x is picked up too

window_rect = ((200, 140), (660, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
include_icon_view_settings = True
arrange_by = None
label_pos = "bottom"
text_size = 13
icon_size = 128
icon_locations = {
    os.path.basename(app): (170, 205),
    "Applications": (490, 205),
}
