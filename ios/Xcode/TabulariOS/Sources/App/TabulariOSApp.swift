import SwiftUI

@main
struct TabulariOSApp: App {
    init() {
        DispatchQueue.main.async {
            _ = tabular_run()
        }
    }

    var body: some Scene {
        WindowGroup {
            Color.black
                .ignoresSafeArea()
        }
    }
}
