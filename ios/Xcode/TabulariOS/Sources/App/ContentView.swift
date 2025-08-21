import SwiftUI

struct ContentView: View {
    @State private var version: String = "(loading)"
    @State private var runResult: Int? = nil

    var body: some View {
        NavigationView {
            Form {
                Section(header: Text("Rust Core")) {
                    HStack {
                        Text("Version")
                        Spacer()
                        Text(version).font(.system(.body, design: .monospaced))
                    }
                    Button("Run Core (experimental)") {
                        DispatchQueue.global().async {
                            let r = tabular_run()
                            DispatchQueue.main.async { self.runResult = Int(r) }
                        }
                    }
                    if let r = runResult { Text("Run exit code: \(r)").font(.footnote) }
                }
                Section(header: Text("Notes")) {
                    Text("UI rendering of egui not yet integrated on iOS.")
                    Text("Add more FFI functions in src/lib.rs as needed.")
                }
            }
            .navigationTitle("Tabular iOS")
        }
        .onAppear { loadVersion() }
    }

    private func loadVersion() {
        if let cstr = tabular_version() { version = String(cString: cstr) }
    }
}

#Preview {
    ContentView()
}
