import AppIntents

struct PlayerShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: ResumeIntent(), phrases: ["Resume \(.applicationName)"], shortTitle: "Resume", systemImageName: "play")
    }
}
