#import "Icon.h"

// The mark (worker/icons/mark.svg): a Greek mu drawn as one stroke with a
// dot on each of its four free ends, white on a rounded square. The
// geometry is the SVG's, in its 32 by 32 box with y down; drawMark maps it
// onto `square` (y up, as AppKit draws).
static void drawMark(NSRect square, NSColor *background, NSColor *foreground) {
    CGFloat k = square.size.width / 32;
    NSPoint (^at)(CGFloat, CGFloat) = ^NSPoint(CGFloat x, CGFloat y) {
        return NSMakePoint(NSMinX(square) + x * k, NSMaxY(square) - y * k);
    };
    [background setFill];
    [[NSBezierPath bezierPathWithRoundedRect:square xRadius:7 * k yRadius:7 * k] fill];
    NSBezierPath *mu = [NSBezierPath bezierPath];
    // M7.5 9.6V22.4: the stem, cut 3.1 short of each pad's centre
    [mu moveToPoint:at(7.5, 9.6)];
    [mu lineToPoint:at(7.5, 22.4)];
    // M7.5 13.5a5.25 5.25 0 0 0 10.5 0: the bowl, a half circle below its ends
    [mu moveToPoint:at(7.5, 13.5)];
    [mu appendBezierPathWithArcWithCenter:at(12.75, 13.5) radius:5.25 * k startAngle:180 endAngle:0 clockwise:NO];
    // M18 9.6V15C18 19.89 20.27 22.52 23.2 23.52: the right arm and its tail,
    // the tail's curve cut 3.1 before the pad's centre along its length
    [mu moveToPoint:at(18, 9.6)];
    [mu lineToPoint:at(18, 15)];
    [mu curveToPoint:at(23.2, 23.52) controlPoint1:at(18, 19.89) controlPoint2:at(20.27, 22.52)];
    mu.lineWidth = 2.75 * k;
    mu.lineJoinStyle = NSLineJoinStyleRound;
    [foreground set];
    [mu stroke];
    // A ring on each free end, like a circuit board's pad: radius 3.1 on its
    // centreline, stroke 1.6, the background showing through the hole.
    CGFloat r = 3.1 * k;
    for (NSValue *v in @[[NSValue valueWithPoint:at(7.5, 6.5)], [NSValue valueWithPoint:at(7.5, 25.5)],
                         [NSValue valueWithPoint:at(18, 6.5)], [NSValue valueWithPoint:at(26.25, 24)]]) {
        NSPoint c = v.pointValue;
        NSBezierPath *pad = [NSBezierPath bezierPathWithOvalInRect:NSMakeRect(c.x - r, c.y - r, 2 * r, 2 * r)];
        pad.lineWidth = 1.6 * k;
        [pad stroke];
    }
}

NSImage *renderIcon(int px) {
    // Draw into a bitmap of exactly px by px pixels: iconutil needs exact
    // pixel sizes per file name, and NSImage drawing alone works in points.
    // Tagged sRGB, so the navy is the site's #1f3a5f exactly.
    NSBitmapImageRep *rep = [[NSBitmapImageRep alloc] initWithBitmapDataPlanes:NULL pixelsWide:px pixelsHigh:px
        bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSCalibratedRGBColorSpace
        bytesPerRow:0 bitsPerPixel:0];
    rep = [rep bitmapImageRepByRetaggingWithColorSpace:[NSColorSpace sRGBColorSpace]];
    rep.size = NSMakeSize(px, px);
    [NSGraphicsContext saveGraphicsState];
    [NSGraphicsContext setCurrentContext:[NSGraphicsContext graphicsContextWithBitmapImageRep:rep]];
    // A margin round the square, as macOS app icons have.
    CGFloat s = px, inset = s * 0.06;
    drawMark(NSMakeRect(inset, inset, s - 2 * inset, s - 2 * inset),
             [NSColor colorWithSRGBRed:0x1f / 255.0 green:0x3a / 255.0 blue:0x5f / 255.0 alpha:1], [NSColor whiteColor]);
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
        drawMark(NSInsetRect(r, 1, 1), color, [NSColor whiteColor]);
        if (dot) {
            // A white badge in the free top right corner, ringed in the
            // state colour so it stands apart from the mu's own pads.
            NSBezierPath *badge = [NSBezierPath bezierPathWithOvalInRect:NSMakeRect(s - 6.6, s - 6.6, 5.2, 5.2)];
            badge.lineWidth = 1.2;
            [[NSColor whiteColor] setFill];
            [color setStroke];
            [badge fill];
            [badge stroke];
        }
        return YES;
    }];
    image.template = NO;
    return image;
}

NSImage *agentIcon(void) {
    CGFloat s = 18;
    NSImage *image = [NSImage imageWithSize:NSMakeSize(s, s) flipped:NO drawingHandler:^BOOL(NSRect r) {
        NSBezierPath *circle = [NSBezierPath bezierPathWithOvalInRect:NSInsetRect(r, 1.5, 1.5)];
        circle.lineWidth = 1.5;
        [[NSColor secondaryLabelColor] setStroke];
        [circle stroke];
        return YES;
    }];
    image.template = YES;
    return image;
}
