import SwiftUI
import AppKit

struct ContentView: View {
    @EnvironmentObject var library: GameLibrary

    private let columns: [GridItem] = [
        GridItem(.flexible(), spacing: 16),
        GridItem(.flexible(), spacing: 16),
        GridItem(.flexible(), spacing: 16),
    ]

    var body: some View {
        VStack(spacing: 12) {
            header
            Divider()
            ScrollView(.vertical) {
                LazyVGrid(columns: columns, spacing: 16) {
                    ForEach(library.games) { game in
                        GameTileView(game: game)
                    }
                }
                .padding(16)
            }
        }
        .frame(minWidth: 920, minHeight: 560)
        .alert(isPresented: $library.showError) {
            Alert(
                title: Text("Error"),
                message: Text(library.errorMessage),
                dismissButton: .default(Text("OK"))
            )
        }
        .sheet(item: $library.pendingImport) { p in
            ImportNlsSheet(pending: p)
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("Sena")
                .font(.title2)
                .bold()

            Spacer()

            Button("Import…") {
                library.importGameFolder()
            }
            .keyboardShortcut("i", modifiers: [.command])

            Button("Refresh") {
                library.refreshValidation()
            }
            .keyboardShortcut("r", modifiers: [.command])
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
    }
}

struct ImportNlsSheet: View {
    @EnvironmentObject var library: GameLibrary
    let pending: PendingImport
    @State private var selectedNls: NlsChoice = .shiftJis

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Import Game")
                .font(.headline)

            Text(pending.title)
                .font(.subheadline)
                .foregroundColor(.secondary)

            Picker("NLS", selection: $selectedNls) {
                ForEach(NlsChoice.allCases) { choice in
                    Text(choice.label).tag(choice)
                }
            }
            .pickerStyle(.segmented)

            HStack {
                Spacer()
                Button("Cancel") {
                    library.pendingImport = nil
                }
                Button("Import") {
                    var updated = pending
                    updated.nls = selectedNls
                    library.commitImport(p: updated)
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 420)
    }
}

struct GameTileView: View {
    @EnvironmentObject var library: GameLibrary
    let game: GameEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ZStack(alignment: .topTrailing) {
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(Color.secondary.opacity(0.35), lineWidth: 1)
                    .background(RoundedRectangle(cornerRadius: 12).fill(Color.secondary.opacity(0.08)))

                if let cover = library.loadCoverImage(game: game) {
                    Image(nsImage: cover)
                        .resizable()
                        .scaledToFill()
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                } else {
                    Text(game.title)
                        .font(.headline)
                        .multilineTextAlignment(.center)
                        .padding(12)
                }

            }
            .frame(height: 160)
            .clipped()

            Text(game.title)
                .font(.headline)
                .lineLimit(2)

            Text(game.rootPath)
                .font(.caption)
                .foregroundColor(.secondary)
                .lineLimit(1)

            Text(NlsChoice.normalized(game.nls).label)
                .font(.caption2)
                .foregroundColor(.secondary)

            HStack {
                Button("Play") {
                    library.launch(game: game)
                }
                .keyboardShortcut(.defaultAction)

                Spacer()

                Menu {
                    Picker("NLS", selection: Binding(
                        get: { NlsChoice.normalized(game.nls) },
                        set: { library.updateNls(game: game, nls: $0) }
                    )) {
                        ForEach(NlsChoice.allCases) { choice in
                            Text(choice.label).tag(choice)
                        }
                    }
                    Button("Reveal in Finder") {
                        library.revealInFinder(game: game)
                    }
                    Button("Remove from Library") {
                        library.remove(game: game)
                    }
                } label: {
                    Text("…")
                        .font(.headline)
                        .frame(width: 28, height: 22)
                }
                .menuStyle(.borderlessButton)
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 16).fill(Color(NSColor.windowBackgroundColor)))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Color.secondary.opacity(0.20)))
    }
}
