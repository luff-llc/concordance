import SwiftUI
import WidgetKit

enum WidgetKinds {
    static let nowPlaying = "com.example.now-playing"
}

struct NowPlayingWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: WidgetKinds.nowPlaying, provider: NowPlayingProvider()) { entry in
            NowPlayingView(entry: entry) // braces in a string: "}"
        }
        .configurationDisplayName("Now Playing")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryCircular])
    }
}

/* struct OldWidget: Widget { kind: "com.example.old" } */

struct DebugWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "DebugWidget", provider: NowPlayingProvider()) { _ in EmptyView() }
    }
}
