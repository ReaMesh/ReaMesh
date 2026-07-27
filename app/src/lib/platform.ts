// Synchronous platform checks from the user agent, also available in browser
// previews without Tauri plugins or permissions.

export const isWindows = /Windows/i.test(navigator.userAgent);
export const isMac = /Macintosh|Mac OS X/i.test(navigator.userAgent);
