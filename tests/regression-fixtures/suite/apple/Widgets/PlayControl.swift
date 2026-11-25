import AppIntents
import WidgetKit

struct PlayControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "com.example.play-pause") {
            ControlWidgetButton(action: PlayPauseIntent()) {
                Label("Play", systemImage: "play.fill")
            }
        }
    }
}
