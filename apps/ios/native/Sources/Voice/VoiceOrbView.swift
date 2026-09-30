import SwiftUI

// MARK: - FlowMode voice orb ("心流" — living LLM presence)
//
// Port of the prototype's `OrbCanvas` + `VoiceOrb` (lingxi-iphone.html). The orb
// is a Canvas render loop driven by `TimelineView(.animation)`: a deep-space
// starfield, a field of orbiting particles, a fluid blob body with a radial core
// + inner glow blobs + specular highlight, three wobbling membrane rings, and a
// thinking arc. Each phase eases toward a SUSTAINED size (no constant pulsing) —
// energy reads as blob wobble + particle motion rather than size jitter.
//
// Both ordinary dictation and Flow Mode render this same canvas inside the
// conversation's inline voice panel.

enum OrbPhase: Equatable { case idle, listening, thinking, speaking }
// MARK: - Orb simulation state (persists across frames, mutated in the draw loop)

private struct OrbParticle { var a: Double; var rad: Double; var sp: Double; var sz: Double; var tw: Double }
private struct OrbStar { var x: Double; var y: Double; var z: Double; var tw: Double }

/// A tiny deterministic RNG so the particle/star field is stable across launches
/// (the prototype seeds with Math.random; we just want a fixed, pleasing spread).
private struct LCG { var s: UInt64; mutating func next() -> Double { s = s &* 6364136223846793005 &+ 1442695040888963407; return Double(s >> 11) / Double(1 << 53) } }

private final class OrbSim {
    var started = false
    var lastTime: Double = 0
    var tm: Double = 0
    var amp: Double = 0.12
    var hueShift: Double = 0
    var baseScale: Double = 1
    var breath: Double = 0
    var parts: [OrbParticle] = []
    var stars: [OrbStar] = []

    init() {
        var r = LCG(s: 0x9E3779B97F4A7C15)
        let tau = Double.pi * 2
        parts = (0..<76).map { _ in
            OrbParticle(a: r.next() * tau,
                        rad: 0.55 + r.next() * 1.5,
                        sp: (0.1 + r.next() * 0.5) * (r.next() < 0.5 ? 1 : -1),
                        sz: 0.6 + r.next() * 1.8,
                        tw: r.next() * tau)
        }
        stars = (0..<90).map { _ in
            OrbStar(x: r.next(), y: r.next(), z: 0.25 + r.next() * 0.75, tw: r.next() * tau)
        }
    }
}
// MARK: - OrbCanvas

struct OrbCanvas: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var phase: OrbPhase
    var cyFrac: CGFloat = 0.40

    @State private var sim = OrbSim()

    var body: some View {
        TimelineView(.animation(paused: reduceMotion)) { timeline in
            Canvas { ctx, size in
                draw(
                    &ctx,
                    size,
                    now: timeline.date.timeIntervalSinceReferenceDate,
                    reduceMotion: reduceMotion
                )
            }
        }
        .accessibilityHidden(true)
    }

    // Closed fluid-blob outline (cx,cy ± harmonic wobble).
    private func blob(_ cx: Double, _ cy: Double, _ base: Double, _ wob: Double, _ seed: Double, _ harm: Double) -> Path {
        var p = Path()
        let tau = Double.pi * 2
        let steps = 132
        let tm = sim.tm
        for i in 0...steps {
            let a = Double(i) / Double(steps) * tau
            let r = base
                + sin(a * 3 + tm * 1.3 + seed) * wob * 0.5
                + sin(a * 5 - tm * 1.9 + seed * 1.7) * wob * 0.32
                + sin(a * 2 + tm * 0.9 + seed * 0.4) * wob * 0.42
                + sin(a * 7 + tm * 2.4) * wob * 0.16 * harm
            let x = cx + cos(a) * r, y = cy + sin(a) * r
            if i == 0 { p.move(to: CGPoint(x: x, y: y)) } else { p.addLine(to: CGPoint(x: x, y: y)) }
        }
        p.closeSubpath()
        return p
    }

    private func draw(
        _ ctx: inout GraphicsContext,
        _ size: CGSize,
        now: Double,
        reduceMotion: Bool
    ) {
        if !sim.started { sim.started = true; sim.lastTime = now }
        let dt = reduceMotion ? 0 : min(0.05, now - sim.lastTime)
        sim.lastTime = now
        sim.tm += dt
        let W = Double(size.width), H = Double(size.height)
        let cx = W / 2, cy = H * Double(cyFrac)
        let baseR = min(W, H) * 0.165

        // Per-phase settled targets (size eases toward target; energy → wobble).
        let baseTarget: Double, breathDepth: Double, breathSpeed: Double, wobTarget: Double
        switch phase {
        case .idle:      baseTarget = 1.0;  breathDepth = 0.03;  breathSpeed = 0.9; wobTarget = 0.10
        case .listening: baseTarget = 1.07; breathDepth = 0.04;  breathSpeed = 1.5; wobTarget = 0.16
        case .thinking:  baseTarget = 1.04; breathDepth = 0.03;  breathSpeed = 1.2; wobTarget = 0.13; sim.hueShift += dt * 40
        case .speaking:  baseTarget = 1.26; breathDepth = 0.035; breathSpeed = 1.8; wobTarget = 0.22
        }
        if reduceMotion {
            sim.baseScale = baseTarget
            sim.amp = wobTarget
        } else {
            sim.baseScale += (baseTarget - sim.baseScale) * min(1, dt * 4.0)
            sim.amp += (wobTarget - sim.amp) * min(1, dt * 3)
        }
        sim.breath += dt * breathSpeed
        let scale = sim.baseScale + (reduceMotion ? 0 : sin(sim.breath) * breathDepth)
        let R = baseR * scale
        let A = max(0, sim.amp)

        // ── deep-space starfield ──
        for s in sim.stars {
            let sx = ((s.x + sim.tm * 0.004 * s.z).truncatingRemainder(dividingBy: 1)) * W
            let sy = s.y * H
            let twk = 0.35 + 0.65 * (0.5 + 0.5 * sin(sim.tm * (1 + s.z * 2) + s.tw))
            let ss = s.z > 0.8 ? 1.6 : 1.0
            let c = Color(okl: 0.78 + s.z * 0.18, 0.03, 250 + s.z * 60, twk * (0.12 + s.z * 0.38))
            ctx.fill(Path(CGRect(x: sx, y: sy, width: ss, height: ss)), with: .color(c))
        }

        // ── orbiting particle field (additive) ──
        ctx.blendMode = .plusLighter
        let conv = phase == .thinking ? 0.6 : 1.0
        for i in sim.parts.indices {
            sim.parts[i].a += sim.parts[i].sp * dt * (phase == .idle ? 0.4 : 1)
            let pt = sim.parts[i]
            let rr = R * pt.rad * conv * (1 + A * 0.25)
            let x = cx + cos(pt.a) * rr * 1.15
            let y = cy + sin(pt.a) * rr
            let tw = 0.4 + 0.6 * (sin(sim.tm * 2 + pt.tw) * 0.5 + 0.5)
            let hue = 250 + 80 * (pt.rad - 0.55) / 1.5 + sim.hueShift
            let c = Color(okl: 0.85, 0.16, hue, tw * (0.5 + A * 0.4))
            let psz = pt.sz * (0.7 + A * 0.5)
            ctx.fill(Path(ellipseIn: CGRect(x: x - psz, y: y - psz, width: psz * 2, height: psz * 2)), with: .color(c))
        }

        let wob = R * (0.05 + A * 0.24)

        // ── blob body: clipped core gradient + inner glow blobs + specular ──
        ctx.blendMode = .normal
        let clipPath = blob(cx, cy, R * 0.94, wob, 0, 1)
        ctx.drawLayer { layer in paintCore(&layer, clip: clipPath, cx: cx, cy: cy, R: R, A: A) }

        paintRings(&ctx, cx: cx, cy: cy, R: R, A: A, wob: wob)
        paintThinkingArc(&ctx, cx: cx, cy: cy, R: R)
        ctx.blendMode = .normal
    }

    /// Clipped blob body: radial core gradient + inner glow blobs + specular.
    private func paintCore(_ layer: inout GraphicsContext, clip: Path, cx: Double, cy: Double, R: Double, A: Double) {
        layer.clip(to: clip)
        let coreH: Double = 282 + 30 * sin(sim.tm * 0.5) + (phase == .thinking ? sim.hueShift * 0.3 : 0)
        let cs0 = Color(okl: 0.97, 0.04, coreH, 0.95)
        let cs1 = Color(okl: 0.80, 0.18, coreH, 0.92)
        let cs2 = Color(okl: 0.58, 0.21, coreH + 20, 0.7)
        let cs3 = Color(okl: 0.40, 0.18, coreH + 30, 0.15)
        let coreGrad = Gradient(stops: [
            .init(color: cs0, location: 0),
            .init(color: cs1, location: 0.32),
            .init(color: cs2, location: 0.7),
            .init(color: cs3, location: 1),
        ])
        let coreRect = CGRect(x: cx - R * 2, y: cy - R * 2, width: R * 4, height: R * 4)
        layer.fill(Path(coreRect), with: .radialGradient(coreGrad, center: CGPoint(x: cx, y: cy - R * 0.28), startRadius: R * 0.05, endRadius: R * 1.15))
        layer.blendMode = .plusLighter
        let glowHues = [260.0, 320.0, 195.0]
        for k in 0..<3 {
            let ya = cy + sin(sim.tm * (1.1 + Double(k) * 0.5) + Double(k)) * R * 0.4 * (0.4 + A)
            let g0 = Color(okl: 0.92, 0.14, glowHues[k] + sim.hueShift, 0.18 + A * 0.22)
            let g1 = Color(okl: 0.90, 0.10, 270, 0)
            let g = Gradient(stops: [.init(color: g0, location: 0), .init(color: g1, location: 1)])
            let rect = CGRect(x: cx - R * 0.9, y: ya - R * 0.9, width: R * 1.8, height: R * 1.8)
            layer.fill(Path(ellipseIn: rect), with: .radialGradient(g, center: CGPoint(x: cx, y: ya), startRadius: 0, endRadius: R * 0.9))
        }
        let spec = Color(okl: 0.99, 0.02, 270, 0.5 + A * 0.3)
        let sr = R * 0.14
        let specRect = CGRect(x: cx - R * 0.28 - sr, y: cy - R * 0.34 - sr, width: sr * 2, height: sr * 2)
        layer.fill(Path(ellipseIn: specRect), with: .color(spec))
    }

    /// Three wobbling membrane rings around the body.
    private func paintRings(_ ctx: inout GraphicsContext, cx: Double, cy: Double, R: Double, A: Double, wob: Double) {
        ctx.blendMode = .plusLighter
        let ringH = [268.0, 322.0, 196.0]
        for k in 0..<3 {
            let path = blob(cx, cy, R * (1.0 + Double(k) * 0.07), wob * (1 + Double(k) * 0.35), Double(k) * 2.3, 1)
            let c = Color(okl: 0.85, 0.17, ringH[k] + sim.hueShift * 0.3, 0.55 - Double(k) * 0.14 + A * 0.2)
            ctx.stroke(path, with: .color(c), lineWidth: 1.6 - Double(k) * 0.45)
        }
    }

    /// A single rotating arc shown while thinking.
    private func paintThinkingArc(_ ctx: inout GraphicsContext, cx: Double, cy: Double, R: Double) {
        guard phase == .thinking else { return }
        let st = sim.tm * 3.2
        var arc = Path()
        arc.addArc(center: CGPoint(x: cx, y: cy), radius: R * 1.32,
                   startAngle: .radians(st), endAngle: .radians(st + .pi * 1.1), clockwise: false)
        ctx.stroke(arc, with: .color(Color(okl: 0.88, 0.16, 260 + sim.hueShift, 0.85)),
                   style: StrokeStyle(lineWidth: 2, lineCap: .round))
    }
}
