// Agents that live on this Mac: each is a myous client state directory
// (~/.myous, ~/.myous-<name>, or one the owner added) with its own key. The
// app reads them through the client's own `myous status --json`, run in that
// directory, and never touches their files. It runs the client as the agent
// only for what the owner asks (accepting a pairing code), with --added-by
// owner so the agent can tell.
#import <Foundation/Foundation.h>

@interface LocalAgent : NSObject
@property (nonatomic, copy) NSString *home;
@property (nonatomic, copy) NSString *binary;      // the client the agent uses, or nil if none was found
@property (nonatomic, copy) NSString *alias;       // nil until the status was read
@property (nonatomic, copy) NSString *npub;
@property (nonatomic, copy) NSString *client;      // "python", "go", "rust", "typescript"
@property (nonatomic, copy) NSString *version;
@property (nonatomic, strong) NSArray<NSDictionary *> *contacts;   // contact_list
@property (nonatomic, strong) NSArray<NSString *> *pending;        // pending_pairings
@property (nonatomic) NSInteger unread;
@property (nonatomic) double lastUsed;
@property (nonatomic, copy) NSString *error;       // why the status could not be read
- (NSString *)shortNpub;                           // npub1q7…x4f
- (NSString *)displayName;                         // alias, or the folder name
@end

typedef void (^AgentsBlock)(NSArray<LocalAgent *> *agents);
typedef void (^CommandBlock)(int status, NSString *output);

@interface Agents : NSObject
/// Candidate homes: ~/.myous and ~/.myous-* that hold a key, plus `extra`.
/// MYOUS_AGENT_HOMES (colon-separated) replaces the search, for tests.
+ (NSArray<NSString *> *)homes:(NSArray<NSString *> *)extra;
/// The client for a home: <home>/venv/bin/myous, <home>/bin/myous, or
/// `myous` on the login shell's PATH.
+ (NSString *)binaryFor:(NSString *)home;
/// Read every home's status in the background; `done` runs on the main
/// thread with one LocalAgent per home (alias nil and `error` set when the
/// client could not answer).
+ (void)read:(NSArray<NSString *> *)homes done:(AgentsBlock)done;
/// Run the client as this agent (MYOUS_HOME set), e.g. accept a code.
+ (void)run:(NSArray<NSString *> *)args as:(LocalAgent *)agent done:(CommandBlock)done;
@end
