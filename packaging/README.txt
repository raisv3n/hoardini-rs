Hoardini for Windows
====================

A fast download manager: segmented downloads with pause/resume, HLS video (.m3u8) saving,
lossless MP4 conversion, and a browser extension that hands downloads over to it.

Requirements: Windows 10 or 11 (64-bit). Nothing else is required.
Optional: ffmpeg, for MP4 conversion and HLS videos with a separate audio track.
          The installer offers to install it for you (via winget). If you skip it, Hoardini
          shows an "Install" button later, and anything waiting for ffmpeg continues by itself
          once it's installed.


INSTALL
-------
1. Extract this zip anywhere (e.g. your Downloads folder).
2. Double-click  install.cmd
   - Hoardini is installed for your user account only; no admin rights are needed.
   - A "Hoardini" shortcut is added to the Start menu.
3. Add the browser extension (Chrome, Edge or Brave):
   - open chrome://extensions  (or edge://extensions)
   - turn on "Developer mode"
   - click "Load unpacked" and choose:
       %LOCALAPPDATA%\Hoardini\extension
     (paste that into the folder picker's address bar)

After installing, you can delete the extracted folder.

"Windows protected your PC" / SmartScreen:
   Hoardini is not code-signed, so Windows may warn the first time. Click "More info",
   then "Run anyway". Source code: {{REPO_URL}}


UPDATE
------
Extract the new version and run its install.cmd. Your downloads, list and settings are kept.
Then click the reload icon on the Hoardini card in chrome://extensions.


UNINSTALL
---------
Double-click uninstall.cmd (from any release zip), or run:
   %LOCALAPPDATA%\Hoardini  ->  delete the folder, and remove the extension in your browser.
Your downloaded files are never touched.


COMMAND LINE
------------
%LOCALAPPDATA%\Hoardini\bin\hoardini.exe <url> [-o dir] [-n connections] [--name file] [--hls]
%LOCALAPPDATA%\Hoardini\bin\hoardini.exe convert <file.ts>...


Licensed under the MIT license (see LICENSE). A port of Hoardini for macOS.
