import { IonButton, IonContent, IonModal, IonTitle } from "@ionic/react";
import { useEffect, useRef } from "react";
import { createFrameLoop } from "@/src/game/frame-loop";
import { useGameStore } from "@/src/stores/game-store";
import { useLingXi } from "@/lib/lingxi-provider";

/// The default editable entry point for a drawn app. Replace the simulation
/// freely; keep the shape:
///
///   - ONE `<canvas className="lingxi-canvas-surface">` filling the viewport
///   - the loop owned here via `createFrameLoop`, started in an effect and
///     stopped in its cleanup
///   - simulation state in a `useRef`, NOT in the store — see game-store.js
///   - overlays (menu, pause, game over) as Ionic components ON TOP of the
///     canvas, so they carry the platform look and get focus handling for free
export function GameScreen() {
  const canvasRef = useRef(null);
  const worldRef = useRef({ x: 0, y: 0, targetX: 0, targetY: 0, pointer: null });
  const { adapter } = useLingXi();

  const phase = useGameStore((state) => state.phase);
  const score = useGameStore((state) => state.score);
  const best = useGameStore((state) => state.best);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const context = canvas.getContext("2d");
    const world = worldRef.current;

    const place = (width, height) => {
      world.targetX = 40 + Math.random() * Math.max(1, width - 80);
      world.targetY = 40 + Math.random() * Math.max(1, height - 80);
    };

    const loop = createFrameLoop(canvas, {
      onResize: ({ width, height, dpr }) => {
        // Draw in CSS pixels: one transform here beats scaling every coordinate.
        context.setTransform(dpr, 0, 0, dpr, 0, 0);
        if (world.x === 0 && world.y === 0) {
          world.x = width / 2;
          world.y = height / 2;
          place(width, height);
        }
      },
      onFrame: ({ dt, width, height }) => {
        const { phase: current } = useGameStore.getState();

        if (current === "playing") {
          const goalX = world.pointer?.x ?? world.x;
          const goalY = world.pointer?.y ?? world.y;
          // Frame-rate independent easing: the exponent makes the result the
          // same at 60 Hz and at 120 Hz.
          const ease = 1 - Math.pow(0.0001, dt);
          world.x += (goalX - world.x) * ease;
          world.y += (goalY - world.y) * ease;

          const dx = world.x - world.targetX;
          const dy = world.y - world.targetY;
          if (Math.hypot(dx, dy) < 28) {
            useGameStore.getState().addScore(1);
            place(width, height);
          }
        }

        context.clearRect(0, 0, width, height);

        context.fillStyle = "#ffb000";
        context.beginPath();
        context.arc(world.targetX, world.targetY, 12, 0, Math.PI * 2);
        context.fill();

        context.fillStyle = "#3dd6a0";
        context.beginPath();
        context.arc(world.x, world.y, 16, 0, Math.PI * 2);
        context.fill();
      },
    });

    // Pointer Events cover touch, trackpad and Pencil in one path; `inputMode`
    // is not always "touch" (iPad with a trackpad reports a fine pointer).
    const toLocal = (event) => {
      const rect = canvas.getBoundingClientRect();
      return { x: event.clientX - rect.left, y: event.clientY - rect.top };
    };
    const onPointerMove = (event) => {
      world.pointer = toLocal(event);
    };
    const onPointerLeave = () => {
      world.pointer = null;
    };

    canvas.addEventListener("pointerdown", onPointerMove);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerLeave);
    canvas.addEventListener("pointercancel", onPointerLeave);

    loop.start();

    return () => {
      loop.stop();
      canvas.removeEventListener("pointerdown", onPointerMove);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerLeave);
      canvas.removeEventListener("pointercancel", onPointerLeave);
    };
  }, []);

  const { start, pause, resume } = useGameStore.getState();

  return (
    <>
      <canvas ref={canvasRef} className="lingxi-canvas-surface" />

      <div
        style={{
          position: "fixed",
          top: "calc(var(--safe-area-top, 0px) + 12px)",
          insetInline: "12px",
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          pointerEvents: "none",
          font: "600 1rem var(--platform-font, system-ui, sans-serif)",
          color: "var(--ion-text-color, #000)",
        }}
      >
        <span>得分 {score}</span>
        <IonButton
          size="small"
          fill="clear"
          style={{ pointerEvents: "auto" }}
          onClick={pause}
          disabled={phase !== "playing"}
        >
          暂停
        </IonButton>
      </div>

      <IonModal
        isOpen={phase !== "playing"}
        backdropDismiss={false}
        // A pause sheet is not a full page; the platform sheet presentation is
        // what makes it read as native on both iOS and Android.
        initialBreakpoint={0.4}
        breakpoints={[0.4]}
      >
        <IonContent className="ion-padding ion-text-center">
          <IonTitle>
            {phase === "menu" ? "准备开始" : phase === "paused" ? "已暂停" : "本局结束"}
          </IonTitle>
          <p>最好成绩 {best}　·　{adapter.ionicMode === "ios" ? "iOS" : "Material"}</p>
          <IonButton expand="block" onClick={phase === "paused" ? resume : start}>
            {phase === "paused" ? "继续" : "开始"}
          </IonButton>
        </IonContent>
      </IonModal>
    </>
  );
}
