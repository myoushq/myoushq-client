// The app icon, drawn in code so the repository needs no binary image:
// a rounded square with a lowercase "m". `MyousWorker --render-icon DIR`
// writes the PNG sizes iconutil wants; make-app.sh turns them into .icns.
#import <AppKit/AppKit.h>
NSImage *renderIcon(int px);
BOOL writeIconSet(NSString *dir, NSError **error);
