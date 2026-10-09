#import "Icon.h"

NSImage *renderIcon(int px) {
    // Draw into a bitmap of exactly px by px pixels: iconutil needs exact
    // pixel sizes per file name, and NSImage drawing alone works in points.
    NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:px pixelsHigh:px
        bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSCalibratedRGBColorSpace
        bytesPerRow:0 bitsPerPixel:0];
    rep.size = NSMakeSize(px, px);
    [NSGraphicsContext saveGraphicsState];
    [NSGraphicsContext setCurrentContext:[NSGraphicsContext graphicsContextWithBitmapImageRep:rep]];
    CGFloat s = px, inset = s * 0.06;
    NSRect rect = NSMakeRect(inset, inset, s - 2 * inset, s - 2 * inset);
    NSBezierPath *path = [NSBezierPath bezierPathWithRoundedRect:rect xRadius:s * 0.2 yRadius:s * 0.2];
    [[NSColor colorWithCalibratedRed:0.13 green:0.17 blue:0.24 alpha:1] setFill];
    [path fill];
    NSFont *font = [NSFont systemFontOfSize:s * 0.62 weight:NSFontWeightBold];
    NSAttributedString *text = [[NSAttributedString alloc] initWithString:@"m"
        attributes:@{NSFontAttributeName: font, NSForegroundColorAttributeName: [NSColor whiteColor]}];
    NSSize ts = [text size];
    [text drawAtPoint:NSMakePoint((s - ts.width) / 2, (s - ts.height) / 2 - s * 0.03)];
    [NSGraphicsContext restoreGraphicsState];
    NSImage *image = [[NSImage alloc] initWithSize:NSMakeSize(px, px)];
    [image addRepresentation:rep];
    return image;
}

BOOL writeIconSet(NSString *dir, NSError **error) {
    if (![[NSFileManager defaultManager] createDirectoryAtPath:dir withIntermediateDirectories:YES attributes:nil error:error])
        return NO;
    // iconutil's expected names: icon_<pt>x<pt>[@2x].png
    NSArray *specs = @[@[@"icon_16x16", @16], @[@"icon_16x16@2x", @32], @[@"icon_32x32", @32], @[@"icon_32x32@2x", @64],
                       @[@"icon_128x128", @128], @[@"icon_128x128@2x", @256], @[@"icon_256x256", @256],
                       @[@"icon_256x256@2x", @512], @[@"icon_512x512", @512], @[@"icon_512x512@2x", @1024]];
    for (NSArray *spec in specs) {
        NSImage *image = renderIcon([spec[1] intValue]);
        NSBitmapImageRep *rep = (NSBitmapImageRep *)image.representations.firstObject;
        NSData *png = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        NSString *path = [dir stringByAppendingPathComponent:[spec[0] stringByAppendingString:@".png"]];
        if (![png writeToFile:path options:NSDataWritingAtomic error:error]) return NO;
    }
    return YES;
}

NSImage *statusIcon(NSColor *color, BOOL dot) {
    CGFloat s = 18;
    NSImage *image = [NSImage imageWithSize:NSMakeSize(s, s) flipped:NO drawingHandler:^BOOL(NSRect r) {
        NSBezierPath *circle = [NSBezierPath bezierPathWithOvalInRect:NSInsetRect(r, 1, 1)];
        [color setFill];
        [circle fill];
        NSFont *font = [NSFont systemFontOfSize:s * 0.62 weight:NSFontWeightBold];
        NSAttributedString *text = [[NSAttributedString alloc] initWithString:@"m"
            attributes:@{NSFontAttributeName: font, NSForegroundColorAttributeName: [NSColor whiteColor]}];
        NSSize ts = [text size];
        [text drawAtPoint:NSMakePoint((s - ts.width) / 2, (s - ts.height) / 2 - s * 0.02)];
        if (dot) {
            [[NSColor whiteColor] setFill];
            [[NSBezierPath bezierPathWithOvalInRect:NSMakeRect(s - 6.5, s - 6.5, 5, 5)] fill];
        }
        return YES;
    }];
    image.template = NO;
    return image;
}
