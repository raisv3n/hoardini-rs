Hoardini for Windows
====================

A fast download manager: segmented downloads with pause/resume, HLS video (.m3u8) saving,
lossless MP4 conversion, and a browser extension that hands downloads over to it.

Requirements: Windows 10 or 11 (64-bit). Nothing else is required.
Optional: ffmpeg, for MP4 conversion and videos with a separate audio track (Vimeo, Facebook HD).
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
   - open chrome://extensions  (or edge://extensions, brave://extensions)
   - turn on "Developer mode"
   - click "Load unpacked" and choose:
       %LOCALAPPDATA%\Hoardini\extension
     (the installer copied it to the clipboard: click the folder picker's address bar,
     press Ctrl+V, then Enter, then "Select Folder")
   - Do NOT choose the "extension" folder in this extracted download. The browser keeps
     loading the extension from the folder you pick, so it would break once you delete this.

After installing, you can delete the extracted folder.
If the extension stopped working after you deleted it: remove the extension, then load it
again from %LOCALAPPDATA%\Hoardini\extension (run install.cmd again if that folder is gone).

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
%LOCALAPPDATA%\Hoardini\bin\hoardini.exe <url> [-o dir] [-n connections] [--name file] [--hls] [--audio url]
%LOCALAPPDATA%\Hoardini\bin\hoardini.exe convert <file.ts>...


Licensed under the MIT license (see LICENSE). A port of Hoardini for macOS.
