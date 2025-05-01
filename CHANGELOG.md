# CHANGELOG

## 0.3.2

- Offer Linux AppImage in addition to bare binary for proper icon/desktop integration with Wayland

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
