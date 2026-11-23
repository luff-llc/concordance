import SwiftUI

struct PlayerView: View {
    @State private var playing = false
    var body: some View {
        Button(playing ? "Pause" : "Play") { playing.toggle() }
            .accessibilityIdentifier("player.play")
            .sensoryFeedback(.impact(weight: .light), trigger: playing)
    }
}
