# Janela do .dmg (dmgbuild -s scripts/package/dmg.py -D app=... -D art=... -D icon=...).
# ruff: noqa: F821
import os

app = defines["app"]  # o dmgbuild põe `defines` aqui
art = defines["art"]

format = "ULFO"
filesystem = "HFS+"
files = [app]
symlinks = {"Aplicativos": "/Applications"}
# Sem `hide_extensions`: o atributo do Finder no .app invalidaria a assinatura.
icon = defines["icon"]
# O dmgbuild junta o `dmg@2x.png` ao lado para as telas Retina.
background = os.path.join(art, "dmg.png")

window_rect = ((240, 160), (660, 420))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
arrange_by = None
icon_size = 112
text_size = 13
icon_locations = {os.path.basename(app): (170, 225), "Aplicativos": (490, 225)}
