# CHANGELOG

## 0.8.0

Add FRES/4W-Resistance support for the Owon XDM2041 and support for the XDM3041/3051
in general. Generously contributed by @jhalilaj in #21 .


## 0.7.3

Ensure range tables are reloaded when a meter connects so that the range selection
is correct also in the initial default mode if the table deviates from the XDM1041.


## 0.7.2

Owon XDM1051 / XDM1251 (150000-count) support, closing (again) [#20](https://github.com/markusdd/rusty_meter/issues/20).

XDM1051/1251 `SYST:BEEP:STATe?` replies `1`/`0` (the 41-series uses ON/NO).
That `1` was read as `AUTO?`, so picking 1000 V in the UI landed on the meter
and then the combo jumped back to Auto and `RANGE?` was never asked. A 0/1
reply is now beep when we asked beep. `RANGE?` is still queried in autorange;
the live window is not applied as a manual range.

XDM1051/1251 range tables follow the user-manual spec (p.46): DC V is
100 mV / 1 V / 10 V / 100 V / 1000 V, DC I is 100 µA … 10 A, resistance goes
to 100 MΩ. AC V, AC I, capacitance, and temperature match the 1041.

## 0.7.1

Owon XDM1051 / XDM1251 (150000-count) support, closing [#20](https://github.com/markusdd/rusty_meter/issues/20).

These meters use a much larger SCPI overload sentinel than the 41-series
(`~1e31` / `9.9e37` vs `1e9`). Values that large are shown as OVERLOAD in every
mode; the 1041 `1e9` flag is still only treated as OL in ohms / diode / continuity
so a 1 GΩ or 1 GHz reading on another meter would graph normally. Overload
samples leave a gap in the main trace, marked with a dashed red overlay so the
graph keeps scrolling. Histogram still ignores them. The graph tab now has a
Reset Graph button, matching Reset Histogram.

## 0.7.0

Add support for Kiprim DC / Owon SPE single-channel power supplies

This release adds a second SCPI device class next to the OWON XDM meters.
You can read live voltage, current and power, set V/I and OVP/OCP, and turn
the output on and off. The graph overlays all three quantities.

On connect the last setpoints are restored, but the output is always turned
off first so a leftover ON at the previous voltage cannot surprise a DUT.
A connect macro may still turn the output on afterwards if you want that.

Tested on a Kiprim DC620S (same SPE SCPI dialect as the Owon SPE series).

Recording stores every channel on a sample, not only the primary reading: PSU
files include V, I, P, output, CV/CC, and protection flags. A DMM capture with
a single value is still Index/Timestamp/Unit/Value.

Also, all dependencies have been bumped to latest versions.

## 0.6.0

Major refactor of the SCPI serial system, status updates now run independent
from scheduling MEAS commands.

Also, a macro system for startup and button-selectable macros has been
introduced. These can be created manually, cloning the current settings
status or they can be made using a record function.

The whole codebase also moved to rust 1.95, egui 0.36.1 and all dependencies
have been moved to latest versions.

## 0.5.0

Add support for Victor (RuoShui) 86B/C/D/E handheld DMMs (read only)

This release adds a whole new device class to Rusty Meter.
These Victor handheld DMMs can be read out, but due to their nature
with a twist knob for the modes they can only be read out
and not remote controlled.

For the B/C/D meters the support is explicitly tested for the newer generation
USB-C type meters. These use a different chip from the old USB HID ones.
The old ones are meant to be supported as well but testers are wanted as
I do not have a meter myself. This release closes #16.

As far as I know, this is the first free program to support the new B/C/D meters
as there was no prior art in the Internet regarding the protocol, which had to
be reverse engineered. If this is useful to you please consider a donation.

## 0.4.6

- bump to egui 0.34.3

## 0.4.5

- fix auto-connect on startup option
- bump to egui 0.34.2

## 0.4.4

- add settings menu option to not send RST on disconnect if user doesn't want it

## 0.4.3

- fix bug that change interval of graph only applies after restart
- make auto scaling of units and measurements selectable per mode
- simplify repaint logic and ensure number box update is not tied to graph update
- upgrade to egui 0.34.1
- upgrade to rust 1.92
- use temporary version of egui-dropdown until PR is included to build against egui 0.34.1

## 0.4.2

- add feature in settings menu to disable unit scaling for CONT mode

## 0.4.1

- move project to rust 1.91.0 (needed for egui_plot fixes and other updates)
- move to egui 0.33.3
- generally update all dependencies
- fix the version tag link in the bottom of the window

## 0.4.0

- histogram functionality, support docking and tabbing in graphing area

This sounds small, but is actually a big upgrade to the graphing functionality of this app.
You can freely arrange the two graphs tabs as overlapping tabs, next to each other, or vertically stacked.
Just grab the tab and start moving it.

You can customize the chart and measurement box font color under File -> Settings.

## 0.3.2

- offer Linux AppImage in addition to bare binary for proper icon/desktop integration with Wayland

## 0.3.1

- release MacOS builds as universal DMG

## 0.3.0

- recording function for CSV, JSON and XLSX
- add MIT license file
- move Linux builds to Rocky Linux 8 for the oldest supported GLIBC base (ressolve #1)

IMPORTANT:
If the program crashes the first time you try to launch the recording function this might be due to old
window states in your program save state. In that case just re-launch it and move it around and close it again cleanly. This should write a clean new state and it should not happen again. This is a drawback
deep down in egui which I cannot do much about right now.

## 0.2.5

- internal: modularize app.rs to make it more maintainable
- add option to reverse graphing scroll direction (most recent value always on left)
- add theming options in settings for graph color, box color and measurement color

## 0.2.4

- internal: re-organize modules
- detect Firmware version to determine if DIOD/CONT are swapped on readback

Please report an issue if this detection does not work for your meter.
If you go to CONT mode and you get thrown back into DIOD mode your meter has
the SCPI bug and the current version check is not sufficient.
Currently it seems it is fixed for V4.3.0 and above, and broken below.

## 0.2.3

- add option to not lock the meter in remote mode
- add mode readback to sync the UI when meter buttons are used
- ensure that on connect we take the current meter mode and sync back beeper and polling rate from UI

Disclaimer: We are NOT syncing back the Range, Beeper State and Polling Rate settings from the meter.
You can change them via buttons on the meter and the values will display completely
fine as we use the RAW mode for that. If this sync back is wanted leave a feature request.
It is NOT possible to sync back changed thresholds via meter buttons for CONT and DIOD modes as there is no SCPI command
for that. Also, the DIOD threshold setting in the UI is purely for the visiual indication in the UI,
it is currently impossibel to set it remotely as Owon has not provided a SCPI command for that either.

Quirk: When looking at the code you will notice CONT and DIOD mode assignments are swapped when read back via FUNC?.
This seems to be a firmware bug of the meter. If this isn't consistent across firmware versions we might need to go
through the trouble to actually distinguish there but we'll see.

Thx to @zach-connolly for suggesting these features and for the donation!

## 0.2.2

- fix icon for Windows executable

## 0.2.1

- ensure proper graph X-axis bounds
- in CONT and DIOD modes flash the measure frame red as a visual indicator

## 0.2.0

- bump dependencies
- make serial comms async
- proper value and unit formatting
- make graph memory depth adjustable
- beeper control
- continuity threshold setting
- connect/disconnect capability without restart
- set graph refresh speed seperate from serial polling speed
