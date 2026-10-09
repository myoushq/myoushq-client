#import "Worker.h"

@implementation Worker
- (instancetype)initWithHome:(NSString *)home {
    if ((self = [super init])) {
        _paths = [[Paths alloc] initWithHome:home];
        _config = [AppConfig readAt:_paths];
        _status = [StatusFile new];
    }
    return self;
}
- (NSString *)name { return self.config.name ?: str(self.status.status[@"alias"]) ?: @"Worker"; }
- (BOOL)isDefault { return [self.paths.home isEqualToString:[Paths defaultHome]]; }
@end
