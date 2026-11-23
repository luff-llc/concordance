import SwiftUI

struct HomeView: View {
    let items: [Item]
    var body: some View {
        List(items) { item in
            Text(item.title)
                .accessibilityIdentifier("home.row.\(item.id)")
        }
        .overlay { if items.isEmpty { Text("Nothing here yet") } }
        .navigationTitle("\(items.count) episodes")
        .accessibilityIdentifier("home.list")
    }
}
