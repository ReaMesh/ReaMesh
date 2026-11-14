// Synchronous platform checks from the user agent, also available in browser
// previews without Tauri plugins or permissions.

export const isWindows = /Windows/i.test(navigator.userAgent);
export const isMac = /Macintosh|Mac OS X/i.test(navigator.userAgent);
export const isLinux = !isWindows && !isMac && /Linux|X11/i.test(navigator.userAgent);

/** The window is undecorated and draws its own title bar (Windows, Linux).
 *  Those platforms have no application menu either, so Settings is a screen
 *  of the main window, opened from the title bar's gear or Ctrl+,. */
export const ownTitleBar = isWindows || isLinux;

/** What "show this folder" is called here. */
export const revealLabel = isWindows ? "showInExplorer" : isLinux ? "showInFileManager" : "showInFinder";
