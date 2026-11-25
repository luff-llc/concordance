import SwiftUI
import WidgetKit

struct WatchNowPlaying: Widget {
    let kind: String = "com.example.watch-now-playing"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: WatchProvider()) { entry in
            WatchNowPlayingView(entry: entry)
        }
        .supportedFamilies([.accessoryCircular, .accessoryCorner])
    }
}
