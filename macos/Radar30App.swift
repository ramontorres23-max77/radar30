import SwiftUI
import AppKit

// MARK: - Design tokens (editorial palette adapted from Awesome-UI-Templates / News & Magazines)

enum Theme {
    static let accent = Color(hex: "F05A28")
    static let accentSoft = Color(hex: "FDE8DC")
    static let ink = Color(hex: "1A1A1A")
    static let muted = Color(hex: "8E8E93")
    static let line = Color(hex: "E5E5EA")
}

extension Color {
    init(hex: String) {
        let clean = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var value: UInt64 = 0
        Scanner(string: clean).scanHexInt64(&value)
        self.init(
            red: Double((value >> 16) & 0xFF) / 255,
            green: Double((value >> 8) & 0xFF) / 255,
            blue: Double(value & 0xFF) / 255
        )
    }
}

// MARK: - Models (radar30/research@1 and radar30/company@1)

struct DateWindow: Decodable { let from: String; let to: String }

struct Engagement: Decodable {
    let score: Int?
    let comments: Int?
    let likes: Int?
    let views: Int?
}

struct Candidate: Decodable, Identifiable {
    let candidateId: String
    let title: String
    let url: String
    let snippet: String?
    let publishedAt: String?
    let primarySource: String
    let sources: [String]
    let finalScore: Double
    let engagement: Engagement?
    let clusterId: String?

    var id: String { candidateId }
    var scoreText: String { String(format: "%.0f", finalScore) }
    var sourceLabel: String { primarySource.uppercased() }
    var metrics: String {
        var parts: [String] = []
        if let score = engagement?.score { parts.append("↑ \(score)") }
        if let comments = engagement?.comments { parts.append("◌ \(comments)") }
        if let views = engagement?.views, views > 0 { parts.append("◉ \(views)") }
        return parts.isEmpty ? "—" : parts.joined(separator: "  ")
    }
}

struct Cluster: Decodable, Identifiable {
    let clusterId: String
    let title: String
    let sources: [String]
    let score: Double
    let uncertainty: String?
    var id: String { clusterId }
}

struct SourceOutcome: Decodable, Identifiable {
    let source: String
    let state: String
    let itemCount: Int
    let elapsedMs: Int?
    let detail: String?
    var id: String { source }
    var color: Color {
        switch state {
        case "ok", "unconfigured": return state == "ok" ? Theme.accent : .secondary
        case "forbidden", "unreachable", "failed": return .orange.opacity(0.75)
        default: return .secondary
        }
    }
}

struct ResearchReport: Decodable {
    let schema: String
    let topic: String
    let primaryEntity: String?
    let intent: String?
    let depth: String?
    let window: DateWindow
    let generatedAt: String?
    let totalFetched: Int
    let candidates: [Candidate]
    let clusters: [Cluster]
    let sourceOutcomes: [SourceOutcome]
    let warnings: [String]?
}

struct Patent: Decodable, Identifiable {
    let number: String
    let title: String
    let assignee: String?
    let priorityDate: String?
    let url: String
    let isCandidateSignal: Bool?
    var id: String { number }
}

struct Job: Decodable, Identifiable {
    let title: String
    let url: String
    let location: String?
    let team: String?
    let publishedAt: String?
    let board: String?
    let isSecretSignal: Bool?
    var id: String { url }
}

struct Finance: Decodable {
    let ticker: String?
    let benchmarkTicker: String?
    let companyChangePct: Double?
    let benchmarkChangePct: Double?
    let lastPrice: String?
    let comparedDays: Int?
}

struct CompanyReport: Decodable {
    let schema: String
    let company: String
    let window: DateWindow?
    let generatedAt: String?
    let patents: [Patent]
    let jobs: [Job]
    let finance: Finance?
    let warnings: [String]?
}

// MARK: - Store / CLI bridge

enum AppSection: String, Identifiable, CaseIterable {
    case research, company, library, status
    var id: String { rawValue }
    var label: String {
        switch self {
        case .research: return "Radar"
        case .company: return "Empresas"
        case .library: return "Biblioteca"
        case .status: return "Status"
        }
    }
    var icon: String {
        switch self {
        case .research: return "waveform.path.ecg"
        case .company: return "building.2"
        case .library: return "books.vertical"
        case .status: return "gearshape"
        }
    }
}

enum CompanySelection: Identifiable {
    case patent(Patent)
    case job(Job)
    var id: String {
        switch self {
        case .patent(let p): return p.id
        case .job(let j): return j.id
        }
    }
}

@MainActor
final class Store: ObservableObject {
    @Published var section: AppSection = .research
    @Published var draftQuery: String = "Rust"
    @Published var report: ResearchReport?
    @Published var companyReport: CompanyReport?
    @Published var selectedCandidate: Candidate?
    @Published var selectedCompanyItem: CompanySelection?
    @Published var isLoading = false
    @Published var errorMessage: String?
    @Published var recentQueries: [String]
    @Published var companyName: String = "Apple"
    @Published var doctorOutput: String = ""

    private let defaults = UserDefaults.standard

    init() {
        self.recentQueries = UserDefaults.standard.stringArray(forKey: "radar30.recent") ?? ["Rust", "AI agents", "Polymarket"]
    }

    private nonisolated static func cliURL() -> URL {
        Bundle.main.url(forAuxiliaryExecutable: "radar30-cli")
            ?? Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/radar30-cli")
    }

    private nonisolated static func log(_ message: String) {
        let url = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/radar30-ui.log")
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let line = "[\(ISO8601DateFormatter().string(from: Date()))] \(message)\n"
        if let handle = FileHandle(forWritingAtPath: url.path) {
            handle.seekToEndOfFile()
            handle.write(Data(line.utf8))
            handle.closeFile()
        } else {
            try? Data(line.utf8).write(to: url)
        }
    }

    private static func invoke(_ args: [String], completion: @escaping (Result<Data, Error>) -> Void) {
        Self.log("invoke: \(args.joined(separator: " "))")
        DispatchQueue.global(qos: .userInitiated).async {
            let process = Process()
            process.executableURL = Self.cliURL()
            process.arguments = args
            let out = Pipe()
            let err = Pipe()
            process.standardOutput = out
            process.standardError = err
            do {
                try process.run()
                let data = out.fileHandleForReading.readDataToEndOfFile()
                let errorData = err.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                if process.terminationStatus == 0 {
                    Self.log("ok: \(data.count) bytes")
                    completion(.success(data))
                } else {
                    let text = String(data: errorData, encoding: .utf8) ?? "O CLI falhou."
                    Self.log("exit \(process.terminationStatus): \(String(text.prefix(200)))")
                    completion(.failure(NSError(domain: "radar30", code: Int(process.terminationStatus),
                                                userInfo: [NSLocalizedDescriptionKey: String(text.prefix(400))])))
                }
            } catch {
                completion(.failure(error))
            }
        }
    }

    private static func decoder() -> JSONDecoder {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }

    private func fixture(for key: String) -> Data? {
        guard let path = ProcessInfo.processInfo.environment["RADAR30_FIXTURE_\(key)"] else { return nil }
        return try? Data(contentsOf: URL(fileURLWithPath: path))
    }

    func search(_ override: String? = nil) {
        let query = (override ?? draftQuery).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return }
        draftQuery = query
        section = .research
        isLoading = true
        errorMessage = nil
        if let data = fixture(for: "RESEARCH") {
            finishSearch(data)
            return
        }
        Self.invoke(["research", query, "--depth", "quick", "--emit", "json"]) { [weak self] result in
            Task { @MainActor in self?.finishSearch(result) }
        }
    }

    private func finishSearch(_ result: Result<Data, Error>) {
        isLoading = false
        switch result {
        case .success(let data): finishSearch(data)
        case .failure(let error): errorMessage = error.localizedDescription
        }
    }

    private func finishSearch(_ data: Data) {
        isLoading = false
        do {
            let decoded = try Self.decoder().decode(ResearchReport.self, from: data)
            report = decoded
            selectedCandidate = decoded.candidates.first
            recentQueries = Array([draftQuery] + recentQueries.filter { $0 != draftQuery }.prefix(4))
            defaults.set(recentQueries, forKey: "radar30.recent")
            errorMessage = nil
        } catch {
            errorMessage = "Não foi possível ler a resposta do radar30: \(error.localizedDescription)"
        }
    }

    func searchCompany() {
        let name = companyName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }
        isLoading = true
        errorMessage = nil
        section = .company
        if let data = fixture(for: "COMPANY") {
            finishCompany(data)
            return
        }
        Self.invoke(["company", name, "--days", "30", "--emit", "json"]) { [weak self] result in
            Task { @MainActor in
                guard let self else { return }
                self.isLoading = false
                switch result {
                case .success(let data): self.finishCompany(data)
                case .failure(let error): self.errorMessage = error.localizedDescription
                }
            }
        }
    }

    private func finishCompany(_ data: Data) {
        isLoading = false
        do {
            companyReport = try Self.decoder().decode(CompanyReport.self, from: data)
            selectedCompanyItem = companyReport?.patents.first.map { CompanySelection.patent($0) }
            errorMessage = nil
        } catch {
            errorMessage = "Não foi possível ler os dados da empresa: \(error.localizedDescription)"
        }
    }

    func runDoctor() {
        isLoading = true
        errorMessage = nil
        section = .status
        Self.invoke(["doctor"]) { [weak self] result in
            Task { @MainActor in
                guard let self else { return }
                self.isLoading = false
                switch result {
                case .success(let data):
                    self.doctorOutput = String(data: data, encoding: .utf8) ?? "Sem saída"
                case .failure(let error):
                    self.errorMessage = error.localizedDescription
                }
            }
        }
    }
}

// MARK: - Shared components

struct SectionLabel: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .semibold))
            .kerning(1.2)
            .foregroundStyle(.secondary)
    }
}

struct Dash: View {
    var body: some View {
        Capsule().fill(Theme.line).frame(width: 22, height: 2)
    }
}

struct SourceBadge: View {
    let text: String
    let color: Color
    var body: some View {
        Text(text.uppercased())
            .font(.system(size: 9, weight: .bold))
            .kerning(0.7)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(color.opacity(0.14))
            .foregroundStyle(color)
            .clipShape(Capsule())
    }
}

struct MetricCard: View {
    let label: String
    let value: String
    let note: String
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            SectionLabel(text: label)
            Text(value)
                .font(.system(size: 30, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(Theme.ink)
            Text(note)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 12))
    }
}

// MARK: - Sidebar

struct SidebarView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 8) {
                Circle().fill(Theme.accent).frame(width: 10, height: 10)
                Text("radar30").font(.title3.bold())
                Spacer()
            }
            SectionLabel(text: "LOCAL RESEARCH · 30 DIAS")

            HStack(spacing: 6) {
                TextField("Tema ou pergunta", text: $store.draftQuery)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { store.search() }
                Button { store.search() } label: {
                    Image(systemName: "magnifyingglass").frame(width: 26)
                }
                .buttonStyle(.borderedProminent).tint(Theme.accent)
                .disabled(store.isLoading)
            }

            List {
                Section {
                    ForEach(AppSection.allCases) { item in
                        Button {
                            store.section = item
                            if item == .status && store.doctorOutput.isEmpty { store.runDoctor() }
                        } label: {
                            Label(item.label, systemImage: item.icon)
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(store.section == item ? Theme.accent : .primary)
                    }
                } header: { SectionLabel(text: "ESPAÇOS") }

                Section {
                    ForEach(store.recentQueries, id: \.self) { item in
                        Button { store.draftQuery = item; store.search(item) } label: {
                            HStack(spacing: 8) {
                                Image(systemName: "clock")
                                Text(item).lineLimit(1)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                } header: { SectionLabel(text: "BUSCAS RECENTES") }
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)

            Dash()
            Text("Keyless por padrão. Dados ficam neste Mac.")
                .font(.caption2).foregroundStyle(.secondary)
        }
        .padding(14)
        .frame(minWidth: 220)
    }
}

// MARK: - Research list

struct ResearchListView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                if let error = store.errorMessage {
                    Text(error).font(.callout).foregroundStyle(.red)
                        .padding(12).background(Color.red.opacity(0.08)).clipShape(RoundedRectangle(cornerRadius: 10))
                }
                if store.isLoading {
                    HStack { ProgressView(); Text("Coletando fontes e pontuando sinais…").foregroundStyle(.secondary) }
                        .frame(maxWidth: .infinity, minHeight: 200)
                } else if let report = store.report {
                    hero(report)
                    metrics(report)
                    SectionLabel(text: "FONTES")
                    sourceChips(report)
                    Dash()
                    SectionLabel(text: "DESTAQUES · \(report.candidates.count) CANDIDATOS")
                    LazyVStack(spacing: 10) {
                        ForEach(report.candidates.prefix(20)) { candidate in
                            Button { store.selectedCandidate = candidate } label: {
                                resultRow(candidate)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    if let warnings = report.warnings, !warnings.isEmpty {
                        DisclosureGroup("Avisos (\(warnings.count))") {
                            ForEach(warnings, id: \.self) { Text($0).font(.caption2).foregroundStyle(.secondary) }
                        }
                        .font(.caption)
                    }
                } else {
                    emptyState
                }
            }
            .padding(22)
        }
        .navigationTitle("Radar")
        .toolbar {
            Button { store.search() } label: { Label("Pesquisar", systemImage: "arrow.clockwise") }
                .disabled(store.isLoading)
        }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionLabel(text: "RADAR DE PESQUISA")
            Text("O que está acontecendo nos últimos 30 dias?")
                .font(.system(size: 36, weight: .bold, design: .serif))
                .foregroundStyle(Theme.ink)
            Text("Escolha um tema na barra lateral. O radar30 consulta fontes públicas sem chaves, cruza sinais e mostra o que merece leitura.")
                .font(.body).foregroundStyle(.secondary)
            Button("Pesquisar \(store.draftQuery)") { store.search() }
                .buttonStyle(.borderedProminent).tint(Theme.accent)
        }
        .padding(.top, 60)
    }

    private func hero(_ report: ResearchReport) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "RADAR · \(report.intent?.uppercased() ?? "ÚLTIMOS 30 DIAS")")
            Text(report.topic)
                .font(.system(size: 42, weight: .bold, design: .serif))
                .foregroundStyle(Theme.ink)
            Text("\(report.window.from) → \(report.window.to)  ·  \(report.depth?.uppercased() ?? "QUICK")  ·  \(report.clusters.count) tópicos")
                .font(.subheadline).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(22)
        .background(
            LinearGradient(colors: [Theme.accent.opacity(0.16), Theme.accent.opacity(0.02)],
                           startPoint: .topLeading, endPoint: .bottomTrailing)
        )
        .clipShape(RoundedRectangle(cornerRadius: 16))
    }

    private func metrics(_ report: ResearchReport) -> some View {
        let ok = report.sourceOutcomes.filter { $0.state == "ok" }.count
        return LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
            MetricCard(label: "ITENS COLETADOS", value: "\(report.totalFetched)", note: "brutos das fontes")
            MetricCard(label: "CANDIDATOS", value: "\(report.candidates.count)", note: "após filtro e RRF")
            MetricCard(label: "CLUSTERS", value: "\(report.clusters.count)", note: "assuntos distintos")
            MetricCard(label: "FONTES OK", value: "\(ok)/\(report.sourceOutcomes.count)", note: "lanes respondendo")
        }
    }

    private func sourceChips(_ report: ResearchReport) -> some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: 8)], spacing: 8) {
            ForEach(report.sourceOutcomes) { outcome in
                HStack(spacing: 6) {
                    Circle().fill(outcome.color).frame(width: 6, height: 6)
                    Text(outcome.source).font(.caption2).lineLimit(1)
                    Spacer()
                    Text("\(outcome.itemCount)").font(.caption2.monospacedDigit()).foregroundStyle(.secondary)
                }
                .padding(.horizontal, 9).padding(.vertical, 6)
                .background(Color(nsColor: .controlBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 8))
            }
        }
    }

    private func resultRow(_ candidate: Candidate) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                SourceBadge(text: candidate.sourceLabel, color: Theme.accent)
                if let date = candidate.publishedAt {
                    Text(date).font(.caption2).foregroundStyle(.secondary)
                }
                Spacer()
                Text(candidate.scoreText)
                    .font(.system(size: 13, weight: .bold, design: .rounded))
                    .foregroundStyle(Theme.accent)
            }
            Text(candidate.title)
                .font(.system(size: 17, weight: .semibold, design: .serif))
                .foregroundStyle(Theme.ink)
                .lineLimit(2)
            Text(candidate.snippet ?? "")
                .font(.caption).foregroundStyle(.secondary).lineLimit(2)
            HStack { Text(candidate.metrics).font(.caption2).foregroundStyle(.secondary); Spacer() }
        }
        .padding(14)
        .background(store.selectedCandidate?.id == candidate.id ? Theme.accent.opacity(0.08) : Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(store.selectedCandidate?.id == candidate.id ? Theme.accent.opacity(0.5) : .clear, lineWidth: 1))
    }
}

// MARK: - Company list

struct CompanyListView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                SectionLabel(text: "COMPANY INTEL")
                Text("Empresa em foco").font(.system(size: 34, weight: .bold, design: .serif)).foregroundStyle(Theme.ink)
                HStack {
                    TextField("Empresa", text: $store.companyName).textFieldStyle(.roundedBorder).onSubmit { store.searchCompany() }
                    Button("Analisar") { store.searchCompany() }.buttonStyle(.borderedProminent).tint(Theme.accent)
                }
                if let report = store.companyReport {
                    if let f = report.finance {
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 150))], spacing: 10) {
                            MetricCard(label: "AÇÃO", value: f.ticker ?? "—", note: f.lastPrice ?? "")
                            MetricCard(label: "\(report.window?.from ?? "") → \(report.window?.to ?? "")",
                                       value: pct(f.companyChangePct), note: "vs \(f.benchmarkTicker ?? "VOO") \(pct(f.benchmarkChangePct))")
                            MetricCard(label: "PATENTES", value: "\(report.patents.count)", note: "publicações US no período")
                            MetricCard(label: "VAGAS", value: "\(report.jobs.count)", note: "sinais de contratação")
                        }
                    }
                    Dash()
                    SectionLabel(text: "PATENTES")
                    ForEach(report.patents) { patent in
                        Button { store.selectedCompanyItem = .patent(patent) } label: {
                            VStack(alignment: .leading, spacing: 5) {
                                Text(patent.title).font(.system(size: 15, weight: .semibold, design: .serif)).lineLimit(2)
                                HStack {
                                    SourceBadge(text: patent.number, color: Theme.accent)
                                    Text(patent.priorityDate ?? "").font(.caption2).foregroundStyle(.secondary)
                                    Spacer()
                                }
                            }.padding(12).background(Color(nsColor: .controlBackgroundColor))
                                .clipShape(RoundedRectangle(cornerRadius: 10))
                        }.buttonStyle(.plain)
                    }
                    if !report.jobs.isEmpty {
                        Dash(); SectionLabel(text: "VAGAS")
                        ForEach(report.jobs.prefix(15)) { job in
                            Button { store.selectedCompanyItem = .job(job) } label: {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(job.title).font(.subheadline.weight(.semibold)).lineLimit(1)
                                    Text("\(job.location ?? "") · \(job.board ?? "")").font(.caption2).foregroundStyle(.secondary)
                                }.frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(10).background(Color(nsColor: .controlBackgroundColor))
                                    .clipShape(RoundedRectangle(cornerRadius: 9))
                            }.buttonStyle(.plain)
                        }
                    }
                }
            }.padding(22)
        }
        .navigationTitle("Empresas")
    }

    private func pct(_ value: Double?) -> String {
        guard let value else { return "—" }
        return String(format: "%+.1f%%", value)
    }
}

// MARK: - Library / Status placeholders with real CLI output

struct LibraryListView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                SectionLabel(text: "BIBLIOTECA LOCAL")
                Text("Tudo o que já foi visto fica no SQLite deste Mac.")
                    .font(.system(size: 30, weight: .bold, design: .serif)).foregroundStyle(Theme.ink)
                Text("Use `radar30 library feed` e `radar30 briefing` no Terminal, ou peça ao assistente via MCP. A interface graphic mostra as pesquisas recentes e os resultados ao vivo.")
                    .font(.body).foregroundStyle(.secondary)
                Dash()
                ForEach(store.recentQueries, id: \.self) { item in
                    Button { store.draftQuery = item; store.search(item) } label: {
                        HStack { Image(systemName: "clock"); Text(item); Spacer(); Image(systemName: "arrow.right") }
                    }.buttonStyle(.plain).padding(12)
                        .background(Color(nsColor: .controlBackgroundColor)).clipShape(RoundedRectangle(cornerRadius: 10))
                }
            }.padding(22)
        }
        .navigationTitle("Biblioteca")
    }
}

struct StatusListView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                SectionLabel(text: "SAÚDE DO SISTEMA")
                Text("Doctor").font(.system(size: 34, weight: .bold, design: .serif)).foregroundStyle(Theme.ink)
                Button("Rodar doctor") { store.runDoctor() }.buttonStyle(.borderedProminent).tint(Theme.accent)
                if store.isLoading { ProgressView() }
                Text(store.doctorOutput.isEmpty ? "—" : store.doctorOutput)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(14).background(Color(nsColor: .controlBackgroundColor))
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                Dash()
                Text("Interface inspirada nos templates News & Magazines do Awesome-UI-Templates. Nenhum asset foi copiado; layout e código são originais.")
                    .font(.caption2).foregroundStyle(.secondary)
            }.padding(22)
        }
        .navigationTitle("Status")
    }
}

// MARK: - Detail pane

struct DetailPaneView: View {
    @EnvironmentObject var store: Store
    var body: some View {
        Group {
            if store.section == .company, let item = store.selectedCompanyItem {
                companyDetail(item)
            } else if let candidate = store.selectedCandidate, store.section == .research {
                candidateDetail(candidate)
            } else {
                VStack(spacing: 10) {
                    Image(systemName: "doc.text.magnifyingglass").font(.system(size: 40)).foregroundStyle(.secondary)
                    Text("Selecione um resultado para ler os detalhes.").foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private func candidateDetail(_ candidate: Candidate) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 8) {
                    SourceBadge(text: candidate.sourceLabel, color: Theme.accent)
                    Text(candidate.publishedAt ?? "").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Text("score \(candidate.scoreText)").font(.caption.weight(.semibold)).foregroundStyle(Theme.accent)
                }
                Text(candidate.title)
                    .font(.system(size: 30, weight: .bold, design: .serif))
                    .foregroundStyle(Theme.ink)
                Dash()
                Text("SINAL CAPTURADO")
                    .font(.system(size: 10, weight: .semibold)).kerning(1.2).foregroundStyle(.secondary)
                Text(candidate.snippet ?? "Sem resumo disponível.")
                    .font(.body).lineSpacing(1.5).foregroundStyle(Theme.ink.opacity(0.85))
                if candidate.sources.count > 1 {
                    Text("Corroborado por: \(candidate.sources.joined(separator: ", "))")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 20)
                if let url = URL(string: candidate.url) {
                    Button { NSWorkspace.shared.open(url) } label: {
                        Label("Abrir fonte no navegador", systemImage: "safari")
                    }.buttonStyle(.borderedProminent).tint(Theme.accent)
                }
                Text(candidate.url).font(.caption2).foregroundStyle(.secondary).textSelection(.enabled)
            }
            .padding(26)
        }
    }

    private func companyDetail(_ item: CompanySelection) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                switch item {
                case .patent(let patent):
                    SourceBadge(text: "PATENTE · \(patent.number)", color: Theme.accent)
                    Text(patent.title).font(.system(size: 28, weight: .bold, design: .serif)).foregroundStyle(Theme.ink)
                    Text(patent.assignee ?? "").font(.subheadline).foregroundStyle(.secondary)
                    Text("Prioridade: \(patent.priorityDate ?? "—")").font(.caption).foregroundStyle(.secondary)
                    if let url = URL(string: patent.url) {
                        Button { NSWorkspace.shared.open(url) } label: { Label("Ver patente", systemImage: "safari") }
                            .buttonStyle(.borderedProminent).tint(Theme.accent)
                    }
                case .job(let job):
                    SourceBadge(text: "VAGA · \(job.board?.uppercased() ?? "JOB")", color: Theme.accent)
                    Text(job.title).font(.system(size: 26, weight: .bold, design: .serif)).foregroundStyle(Theme.ink)
                    Text("\(job.location ?? "") · \(job.publishedAt ?? "")").font(.caption).foregroundStyle(.secondary)
                    if let url = URL(string: job.url) {
                        Button { NSWorkspace.shared.open(url) } label: { Label("Ver vaga", systemImage: "safari") }
                            .buttonStyle(.borderedProminent).tint(Theme.accent)
                    }
                }
            }.padding(26)
        }
    }
}

// MARK: - App

@main
struct Radar30App: App {
    @StateObject private var store = Store()

    var body: some Scene {
        WindowGroup {
            NavigationSplitView {
                SidebarView()
            } content: {
                switch store.section {
                case .research: ResearchListView()
                case .company: CompanyListView()
                case .library: LibraryListView()
                case .status: StatusListView()
                }
            } detail: {
                DetailPaneView()
            }
            .environmentObject(store)
            .onAppear {
                if ProcessInfo.processInfo.environment["RADAR30_FIXTURE_RESEARCH"] != nil {
                    store.search()
                }
            }
            .frame(minWidth: 1100, minHeight: 720)
        }
        .windowStyle(.titleBar)
        .defaultSize(width: 1280, height: 820)
    }
}
