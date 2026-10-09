// Icons drawn in code, so the repository needs no binary image: the app
// icon (a rounded square with a lowercase "m"; `myous --render-icon DIR`
// writes the PNG sizes iconutil wants, make-app.sh turns them into .icns)
// and the menu bar icon (the same mark in a state colour).
#import <AppKit/AppKit.h>
NSImage *renderIcon(int px);
BOOL writeIconSet(NSString *dir, NSError **error);
/// The menu bar icon: a filled circle in `color` with a white "m"; `dot`
/// adds a small white dot (a request in progress).
NSImage *statusIcon(NSColor *color, BOOL dot);
