import SwiftUI

struct ContentView: View {
    @EnvironmentObject var library: GameLibrary

    @State private var isLaunching: Bool = false

    private let columns: [GridItem] = [
        GridItem(.adaptive(minimum: 150, maximum: 210), spacing: 10),
    ]

    var body: some View {
        ZStack {
            VStack(spacing: 8) {
                header
                Divider()
                Text("Copy games into Files → On My iPhone → Sena → sena, then tap Rescan.")
                    .font(.footnote)
                    .foregroundColor(.secondary)
                    .padding(.horizontal, 12)
                ScrollView(.vertical) {
                    LazyVGrid(columns: columns, spacing: 10) {
                        ForEach(library.games) { game in
                            GameTileView(game: game, isLaunching: $isLaunching)
                        }
                    }
                    .padding(10)
                }
            }
            .alert(isPresented: $library.showError) {
                Alert(
                    title: Text("Error"),
                    message: Text(library.errorMessage),
                    dismissButton: .default(Text("OK"))
                )
            }

            if isLaunching {
                Color.black.opacity(0.35).ignoresSafeArea()
                VStack(spacing: 12) {
                    ProgressView()
                    Text("Launching…")
                        .foregroundColor(.white)
                }
                .padding(18)
                .background(RoundedRectangle(cornerRadius: 12).fill(Color.black.opacity(0.6)))
            }
        }
        .fullScreenCover(item: $library.activeGame, onDismiss: {
            isLaunching = false
        }) { game in
            SenaPlayerScreen(game: game)
                .environmentObject(library)
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text("Sena")
                .font(.headline)

            Spacer()

            Picker("NLS", selection: $library.defaultNls) {
                ForEach(NlsChoice.allCases) { choice in
                    Text(choice.label).tag(choice)
                }
            }
            .pickerStyle(.menu)

            Button("Rescan") {
                library.rescanFromDocuments()
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
    }
}

struct GameTileView: View {
    @EnvironmentObject var library: GameLibrary

    let game: GameEntry
    @Binding var isLaunching: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack(alignment: .topTrailing) {
                RoundedRectangle(cornerRadius: 12)
                    .fill(Color(UIColor.secondarySystemBackground))

                if let cover = library.loadCoverImage(game: game) {
                    Image(uiImage: cover)
                        .resizable()
                        .scaledToFill()
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                } else {
                    VStack(spacing: 0) {
                        Text(game.title)
                            .font(.headline)
                            .multilineTextAlignment(.center)
                            .padding(10)
                        Spacer()
                    }
                }

            }
            .frame(height: 104)

            Text(game.title)
                .font(.subheadline)
                .lineLimit(2)

            Text(game.rootPath)
                .font(.caption2)
                .foregroundColor(.secondary)
                .lineLimit(1)

            HStack(spacing: 8) {
                Button("Play") {
                    isLaunching = true
                    DispatchQueue.main.async {
                        library.launch(game: game)
                    }
                }

                Spacer()

                Menu(NlsChoice.normalized(game.nls).label) {
                    ForEach(NlsChoice.allCases) { choice in
                        Button(choice.label) {
                            library.updateNls(game: game, nls: choice)
                        }
                    }
                    Divider()
                    Button("Remove") {
                        library.remove(game: game)
                    }
                }
            }
        }
        .padding(8)
    }
}
