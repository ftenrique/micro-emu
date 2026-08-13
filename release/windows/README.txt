CODEX MICRO 1.0.0 - WINDOWS X64
================================

INSTALL
1. Extract this ZIP to a folder.
2. Double-click Install.cmd.
3. Confirm the plugin installation when Stream Deck opens.

The installer uses your local application-data and Startup folders. It does not
need administrator access. The bridge automatically detects an RP2040 serial
port when one is available and still runs for the Stream Deck plugin without it.

FIRMWARE (OPTIONAL - only if you have an RP2040 Zero board)
This step flashes the Codex Micro firmware onto the board. Skip it if you only
use the Stream Deck plugin.
1. Hold BOOTSEL on the board and connect it via a USB data cable.
2. Release BOOTSEL when the RPI-RP2 drive appears in File Explorer.
3. Double-click Flash-Firmware.cmd in this folder.
4. The board reboots automatically and the RPI-RP2 drive disappears. Restart
   the bridge (or relaunch Codex Micro) for it to detect the board.

The prebuilt firmware image codex_micro_rp2040_bridge.uf2 is included in this
folder. A standalone copy is also published as a separate release asset for
re-flashing without re-downloading the whole bundle.

UNINSTALL
Double-click Uninstall.cmd, then remove the Codex Micro plugin in Stream Deck.

Project: https://github.com/ftenrique/micro-emu
