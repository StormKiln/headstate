#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>
#include <stdbool.h>

typedef void (*HeadstateClick)(const char *);
static HeadstateClick clickHandler;
API_AVAILABLE(macos(10.14))
@interface HeadstateNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end
@implementation HeadstateNotificationDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
 didReceiveNotificationResponse:(UNNotificationResponse *)response
         withCompletionHandler:(void (^)(void))completion {
    if ([response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier]) {
        id value = response.notification.request.content.userInfo[@"headstatePr"];
        NSString *payload = [value isKindOfClass:NSString.class] ? value : nil;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (clickHandler) clickHandler(payload.UTF8String);
            completion();
        });
    } else { completion(); }
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
      willPresentNotification:(UNNotification *)notification
        withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    if (@available(macOS 11.0, *)) {
        completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
    } else {
        completion(UNNotificationPresentationOptionAlert);
    }
}
@end

bool headstate_notifications_supported(void) {
    // The plugin can deliver dev notifications using Terminal's identity;
    // UN requires our own application bundle. Keep that fallback reachable.
    if (@available(macOS 10.14, *)) return NSBundle.mainBundle.bundleIdentifier.length > 0;
    return false;
}

void headstate_notifications_init(HeadstateClick callback) {
    if (@available(macOS 10.14, *)) {
        clickHandler = callback;
        // UN's delegate is weak. Keep one strong process-lifetime reference.
        static HeadstateNotificationDelegate *delegate;
        static dispatch_once_t once;
        dispatch_once(&once, ^{ delegate = [HeadstateNotificationDelegate new]; });
        // Command-line/dev executables lack the bundle required by UN. Never
        // initialize it there: Apple's currentNotificationCenter can assert.
        if (NSBundle.mainBundle.bundleIdentifier) {
            UNUserNotificationCenter.currentNotificationCenter.delegate = delegate;
        }
    }
}
void headstate_notify(const char *titleBytes, const char *bodyBytes, const char *identityBytes) {
    if (@available(macOS 10.14, *)) {
        NSString *title = [NSString stringWithUTF8String:titleBytes];
        NSString *body = [NSString stringWithUTF8String:bodyBytes];
        NSString *identity = [NSString stringWithUTF8String:identityBytes];
        if (!NSBundle.mainBundle.bundleIdentifier) { NSLog(@"Headstate PR notifications require an application bundle"); return; }
        UNUserNotificationCenter *center = UNUserNotificationCenter.currentNotificationCenter;
        [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert completionHandler:^(BOOL granted, NSError *error) {
            if (!granted) { NSLog(@"Headstate notification authorization denied: %@", error); return; }
            UNMutableNotificationContent *content = [UNMutableNotificationContent new];
            content.title = title;
            content.body = body;
            content.userInfo = @{@"headstatePr": identity};
            UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:NSUUID.UUID.UUIDString content:content trigger:nil];
            [center addNotificationRequest:request withCompletionHandler:^(NSError *deliveryError) {
                if (deliveryError) NSLog(@"Headstate notification delivery failed: %@", deliveryError);
            }];
        }];
    }
}
void headstate_about(void) {
    NSString *url = @"https://github.com/StormKiln/headstate";
    NSAttributedString *credits = [[NSAttributedString alloc] initWithString:@"Headstate on GitHub" attributes:@{NSLinkAttributeName: [NSURL URLWithString:url]}];
    [NSApp orderFrontStandardAboutPanelWithOptions:@{NSAboutPanelOptionCredits: credits}];
}
