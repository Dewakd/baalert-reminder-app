import {
  type CSSProperties,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { LogicalSize, PhysicalPosition } from "@tauri-apps/api/dpi";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import "./App.css";
import petIdle from "../src-tauri/resources/character/idle/Front_Idle_01.png";

type PetOverlayPayload = {
  revision: number;
  title: string;
  message: string;
  showAfterSeconds: number;
  visibleForSeconds: number;
  showBubble: boolean;
  petSize: number;
  bubbleStyle: BubbleStyle;
  reminderAnimation: string;
  soundDataUrl: string | null;
  soundVolume: number;
  darkMode: boolean;
};

type PetOverlayFrameSet = {
  idle: string[];
  runLeft: string[];
  runRight: string[];
  hover: string[];
  reminder: string[];
};

type PetMotion = "idle" | "runLeft" | "runRight" | "hover" | "reminder";
type PetBubbleSide = "left" | "right";
type PetWindowLayoutMode = "pet" | PetBubbleSide;
type PetWindowLayout = {
  mode: PetWindowLayoutMode;
  petSize: number;
  width: number;
  height: number;
  petX: number;
  petY: number;
  bubbleHeight: number;
};

const MAX_REMINDER_MESSAGE_LENGTH = 240;

const DEFAULT_PET_OVERLAY: PetOverlayPayload = {
  revision: 0,
  title: "Baalert reminder",
  message: "You have something coming up soon.",
  showAfterSeconds: 0,
  visibleForSeconds: 10,
  showBubble: true,
  petSize: 152,
  bubbleStyle: "lime",
  reminderAnimation: "idle",
  soundDataUrl: null,
  soundVolume: 70,
  darkMode: false,
};

function normalizePetOverlayPayload(
  parsed: Partial<PetOverlayPayload>,
): PetOverlayPayload {
  const bubbleStyle = ["lime", "pink", "yellow", "cyan"].includes(
    parsed.bubbleStyle ?? "",
  )
    ? (parsed.bubbleStyle as BubbleStyle)
    : DEFAULT_PET_OVERLAY.bubbleStyle;

  const visibleForSeconds = Number(parsed.visibleForSeconds);

  return {
    ...DEFAULT_PET_OVERLAY,
    ...parsed,
    revision: Number(parsed.revision) || Date.now(),
    title:
      typeof parsed.title === "string"
        ? parsed.title
        : DEFAULT_PET_OVERLAY.title,
    message:
      typeof parsed.message === "string"
        ? parsed.message
        : DEFAULT_PET_OVERLAY.message,
    petSize: Math.min(224, Math.max(96, Number(parsed.petSize) || 152)),
    showAfterSeconds: Math.min(
      3600,
      Math.max(0, Number(parsed.showAfterSeconds) || 0),
    ),
    visibleForSeconds:
      visibleForSeconds === 0
        ? 0
        : Math.min(3600, Math.max(2, visibleForSeconds || 10)),
    bubbleStyle,
    soundDataUrl:
      typeof parsed.soundDataUrl === "string" ? parsed.soundDataUrl : null,
    soundVolume: Math.min(
      100,
      Math.max(
        0,
        parsed.soundVolume === undefined
          ? DEFAULT_PET_OVERLAY.soundVolume
          : Number(parsed.soundVolume) || 0,
      ),
    ),
  };
}

function createPetWindowLayout(
  mode: PetWindowLayoutMode,
  petSize: number,
  bubbleHeight = 92,
): PetWindowLayout {
  const width = mode === "pet" ? petSize + 28 : petSize + 400;
  const height =
    mode === "pet"
      ? petSize + 28
      : Math.max(
          petSize + 50,
          bubbleHeight + Math.round(petSize * 0.34) + 38,
        );

  return {
    mode,
    petSize,
    width,
    height,
    petX: mode === "left" ? width - petSize - 14 : 14,
    petY: height - petSize - 18,
    bubbleHeight: mode === "pet" ? 0 : bubbleHeight,
  };
}

function bundledFrames(modules: Record<string, unknown>) {
  return Object.entries(modules)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([, source]) => source as string);
}

const BUNDLED_IDLE_FRAMES = bundledFrames(
  import.meta.glob("../src-tauri/resources/character/idle/*.png", {
    eager: true,
    query: "?url",
    import: "default",
  }),
);
const BUNDLED_RUN_LEFT_FRAMES = bundledFrames(
  import.meta.glob("../src-tauri/resources/character/run-left/*.png", {
    eager: true,
    query: "?url",
    import: "default",
  }),
);
const BUNDLED_RUN_RIGHT_FRAMES = bundledFrames(
  import.meta.glob("../src-tauri/resources/character/run-right/*.png", {
    eager: true,
    query: "?url",
    import: "default",
  }),
);

function readPetOverlayPayload(): PetOverlayPayload {
  const encoded = new URLSearchParams(window.location.search).get("payload");
  if (!encoded) return DEFAULT_PET_OVERLAY;

  try {
    const standardBase64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const padded = standardBase64.padEnd(
      standardBase64.length + ((4 - (standardBase64.length % 4)) % 4),
      "=",
    );
    const binary = window.atob(padded);
    const bytes = Uint8Array.from(binary, (character) =>
      character.charCodeAt(0),
    );
    return normalizePetOverlayPayload(
      JSON.parse(new TextDecoder().decode(bytes)),
    );
  } catch {
    return DEFAULT_PET_OVERLAY;
  }
}

function initialPetFrames(): PetOverlayFrameSet {
  const idle = BUNDLED_IDLE_FRAMES.length ? BUNDLED_IDLE_FRAMES : [petIdle];
  return {
    idle,
    runLeft: BUNDLED_RUN_LEFT_FRAMES.length
      ? BUNDLED_RUN_LEFT_FRAMES
      : idle,
    runRight: BUNDLED_RUN_RIGHT_FRAMES.length
      ? BUNDLED_RUN_RIGHT_FRAMES
      : idle,
    hover: idle,
    reminder: idle,
  };
}

// Webview overlay used on Windows and Linux; macOS uses the native panel.
function PetOverlay() {
  const initialPayload = useRef(readPetOverlayPayload()).current;
  const appWindow = useRef(
    "__TAURI_INTERNALS__" in window ? getCurrentWindow() : null,
  ).current;
  const [payload, setPayload] = useState(initialPayload);
  const [frames, setFrames] = useState<PetOverlayFrameSet>(initialPetFrames);
  const [frameIndex, setFrameIndex] = useState(0);
  const [bubbleVisible, setBubbleVisible] = useState(false);
  const [bubbleSide, setBubbleSide] = useState<PetBubbleSide>("right");
  const [layoutRefresh, setLayoutRefresh] = useState(0);
  const [interactionMotion, setInteractionMotion] = useState<
    Exclude<PetMotion, "idle" | "reminder"> | undefined
  >();
  const dragging = useRef(false);
  const movedDuringDrag = useRef(false);
  const lastWindowPosition = useRef<{ x: number; y: number } | undefined>(
    undefined,
  );
  const dragOriginPosition = useRef<{ x: number; y: number } | undefined>(
    undefined,
  );
  const releasePollTimer = useRef<number | undefined>(undefined);
  const bubbleElement = useRef<HTMLDivElement | null>(null);
  const layoutRequest = useRef(0);
  const windowLayout = useRef(
    createPetWindowLayout("pet", initialPayload.petSize),
  );

  const motion: PetMotion =
    interactionMotion ?? (bubbleVisible ? "reminder" : "idle");
  const activeFrames = frames[motion].length ? frames[motion] : frames.idle;

  useEffect(() => {
    if (!appWindow) return;

    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void appWindow
      .listen<PetOverlayPayload>("pet-overlay-update", (event) => {
        setPayload(normalizePetOverlayPayload(event.payload));
      })
      .then((stopListening) => {
        if (cancelled) {
          stopListening();
        } else {
          unlisten = stopListening;
        }
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appWindow]);

  useEffect(() => {
    if (!payload.soundDataUrl || payload.soundVolume <= 0) return;

    const audio = new Audio(payload.soundDataUrl);
    audio.volume = payload.soundVolume / 100;
    void audio.play().catch(() => undefined);
    return () => {
      audio.pause();
      audio.src = "";
    };
  }, [payload.revision, payload.soundDataUrl, payload.soundVolume]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;

    let cancelled = false;

    void invoke<PetOverlayFrameSet>("get_pet_overlay_frames", {
      reminderAnimation: payload.reminderAnimation,
    })
      .then((loaded) => {
        if (cancelled) return;
        const fallback =
          loaded.idle.length
            ? loaded.idle
            : loaded.runLeft.length
              ? loaded.runLeft
              : loaded.runRight.length
                ? loaded.runRight
                : loaded.hover.length
                  ? loaded.hover
                  : loaded.reminder;
        if (!fallback.length) return;

        const selectedCoreFrames =
          payload.reminderAnimation === "run-left"
            ? loaded.runLeft
            : payload.reminderAnimation === "run-right"
              ? loaded.runRight
              : payload.reminderAnimation === "hover"
                ? loaded.hover
                : payload.reminderAnimation === "idle"
                  ? loaded.idle
                  : loaded.reminder;

        setFrames({
          idle: loaded.idle.length ? loaded.idle : fallback,
          runLeft: loaded.runLeft.length ? loaded.runLeft : fallback,
          runRight: loaded.runRight.length ? loaded.runRight : fallback,
          hover: loaded.hover.length ? loaded.hover : fallback,
          reminder: selectedCoreFrames.length ? selectedCoreFrames : fallback,
        });
      })
      .catch(() => {
        // Bundled frames keep the built-in character animated in web previews.
      });

    return () => {
      cancelled = true;
    };
  }, [payload.reminderAnimation, payload.revision]);

  useEffect(() => {
    if (!payload.showBubble) {
      setBubbleVisible(false);
      return;
    }

    let hideTimer: number | undefined;
    const showBubble = () => {
      setBubbleVisible(true);
      if (payload.visibleForSeconds > 0) {
        hideTimer = window.setTimeout(
          () => setBubbleVisible(false),
          payload.visibleForSeconds * 1000,
        );
      }
    };
    const showTimer =
      payload.showAfterSeconds === 0
        ? (showBubble(), undefined)
        : window.setTimeout(showBubble, payload.showAfterSeconds * 1000);
    if (payload.showAfterSeconds > 0) setBubbleVisible(false);

    return () => {
      if (showTimer !== undefined) window.clearTimeout(showTimer);
      if (hideTimer !== undefined) window.clearTimeout(hideTimer);
    };
  }, [
    payload.revision,
    payload.showAfterSeconds,
    payload.showBubble,
    payload.visibleForSeconds,
  ]);

  const applyWindowLayout = useCallback(
    async (showBubble: boolean) => {
      if (!appWindow) return;

      const request = ++layoutRequest.current;
      try {
        const [position, scaleFactor, monitor] = await Promise.all([
          appWindow.outerPosition(),
          appWindow.scaleFactor(),
          currentMonitor(),
        ]);
        if (request !== layoutRequest.current) return;

        const previousLayout = windowLayout.current;
        const petScreenX = position.x + previousLayout.petX * scaleFactor;
        const petScreenY = position.y + previousLayout.petY * scaleFactor;
        let nextLayout = createPetWindowLayout("pet", payload.petSize);

        if (showBubble) {
          const bubbleHeight = Math.max(
            92,
            Math.ceil(bubbleElement.current?.getBoundingClientRect().height ?? 92),
          );
          const rightLayout = createPetWindowLayout(
            "right",
            payload.petSize,
            bubbleHeight,
          );
          const leftLayout = createPetWindowLayout(
            "left",
            payload.petSize,
            bubbleHeight,
          );
          const workArea = monitor?.workArea;
          const rightX = petScreenX - rightLayout.petX * scaleFactor;
          const leftX = petScreenX - leftLayout.petX * scaleFactor;
          const workLeft = workArea?.position.x ?? Number.NEGATIVE_INFINITY;
          const workRight = workArea
            ? workArea.position.x + workArea.size.width
            : Number.POSITIVE_INFINITY;
          const rightFits =
            rightX + rightLayout.width * scaleFactor <= workRight;
          const leftFits = leftX >= workLeft;
          const side: PetBubbleSide =
            rightFits || !leftFits ? "right" : "left";
          nextLayout = side === "right" ? rightLayout : leftLayout;
        }

        let nextX = petScreenX - nextLayout.petX * scaleFactor;
        let nextY = petScreenY - nextLayout.petY * scaleFactor;
        if (monitor) {
          const workLeft = monitor.workArea.position.x;
          const workTop = monitor.workArea.position.y;
          const workRight = workLeft + monitor.workArea.size.width;
          const workBottom = workTop + monitor.workArea.size.height;
          nextX = Math.min(
            Math.max(nextX, workLeft),
            workRight - nextLayout.width * scaleFactor,
          );
          nextY = Math.min(
            Math.max(nextY, workTop),
            workBottom - nextLayout.height * scaleFactor,
          );
        }

        if (request !== layoutRequest.current) return;
        windowLayout.current = nextLayout;
        if (nextLayout.mode !== "pet") setBubbleSide(nextLayout.mode);
        await appWindow.setSize(
          new LogicalSize(nextLayout.width, nextLayout.height),
        );
        await appWindow.setPosition(
          new PhysicalPosition(Math.round(nextX), Math.round(nextY)),
        );
      } catch {
        // Keep the last valid layout if the monitor changes during the update.
      }
    },
    [appWindow, payload.petSize],
  );

  useEffect(() => {
    void applyWindowLayout(bubbleVisible);
  }, [applyWindowLayout, bubbleVisible, layoutRefresh]);

  useEffect(() => {
    setFrameIndex(0);
    if (activeFrames.length <= 1) return;

    const frameTimer = window.setInterval(() => {
      setFrameIndex((current) => (current + 1) % activeFrames.length);
    }, 90);

    return () => window.clearInterval(frameTimer);
  }, [activeFrames, motion]);

  const finishDrag = useCallback((openOnClick = true) => {
    if (!dragging.current) return;

    if (releasePollTimer.current !== undefined) {
      window.clearTimeout(releasePollTimer.current);
      releasePollTimer.current = undefined;
    }
    const shouldOpenDashboard = openOnClick && !movedDuringDrag.current;
    dragging.current = false;
    dragOriginPosition.current = undefined;
    setInteractionMotion(undefined);
    setLayoutRefresh((current) => current + 1);
    if (shouldOpenDashboard) void invoke("open_dashboard");
  }, []);

  useEffect(() => {
    const handlePointerUp = () => finishDrag();
    const handlePointerCancel = () => finishDrag(false);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("mouseup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerCancel);

    return () => {
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("mouseup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerCancel);
      if (releasePollTimer.current !== undefined) {
        window.clearTimeout(releasePollTimer.current);
      }
    };
  }, [finishDrag]);

  useEffect(() => {
    if (!appWindow) return;

    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void appWindow
      .outerPosition()
      .then((position) => {
        lastWindowPosition.current = { x: position.x, y: position.y };
      })
      .catch(() => undefined);

    void appWindow
      .onMoved(({ payload: position }) => {
        if (!dragging.current) {
          lastWindowPosition.current = { x: position.x, y: position.y };
          return;
        }

        const previous = lastWindowPosition.current;
        const origin = dragOriginPosition.current ?? previous ?? position;
        dragOriginPosition.current = { x: origin.x, y: origin.y };
        lastWindowPosition.current = { x: position.x, y: position.y };

        const distance = Math.hypot(
          position.x - origin.x,
          position.y - origin.y,
        );
        if (distance >= 8) movedDuringDrag.current = true;
        if (!previous || position.x === previous.x) return;
        setInteractionMotion(position.x < previous.x ? "runLeft" : "runRight");
      })
      .then((stopListening) => {
        if (cancelled) {
          stopListening();
        } else {
          unlisten = stopListening;
        }
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appWindow]);

  const startDrag = () => {
    if (!appWindow || dragging.current) return;

    dragging.current = true;
    movedDuringDrag.current = false;
    dragOriginPosition.current = lastWindowPosition.current
      ? { ...lastWindowPosition.current }
      : undefined;
    setInteractionMotion("runRight");

    const watchForRelease = () => {
      void invoke<boolean | null>("is_primary_mouse_button_pressed")
        .then((pressed) => {
          if (!dragging.current || pressed === null) return;
          if (!pressed) {
            finishDrag();
            return;
          }
          releasePollTimer.current = window.setTimeout(watchForRelease, 40);
        })
        .catch(() => undefined);
    };

    void appWindow.startDragging().catch(() => finishDrag(false));
    releasePollTimer.current = window.setTimeout(watchForRelease, 40);
  };

  return (
    <div
      className={`pet-window-root pet-window-${payload.bubbleStyle} pet-window-bubble-${bubbleSide}${payload.darkMode ? " pet-window-dark" : ""}`}
      style={{ "--pet-size": `${payload.petSize}px` } as CSSProperties}
    >
      <img
        src={activeFrames[frameIndex % activeFrames.length]}
        alt=""
        className="pet-window-character"
        draggable={false}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          startDrag();
        }}
        onPointerCancel={() => finishDrag(false)}
        onPointerUp={() => finishDrag()}
        onPointerEnter={() => {
          if (!dragging.current) setInteractionMotion("hover");
        }}
        onPointerLeave={() => {
          if (!dragging.current) setInteractionMotion(undefined);
        }}
      />
      {bubbleVisible && (
        <div ref={bubbleElement} className="pet-window-bubble" role="status">
          <button
            type="button"
            className="pet-window-bubble-close"
            aria-label="Dismiss reminder"
            onClick={() => setBubbleVisible(false)}
          >
            x
          </button>
          <small>{payload.title}</small>
          <span>{payload.message}</span>
        </div>
      )}
    </div>
  );
}

// ─── Main Dashboard ──────────────────────────────────────────────────────────
type NavPage = "dashboard" | "calendar" | "reminders" | "settings";

type Reminder = {
  id: string;
  title: string;
  message: string;
  intervalValue: number;
  intervalUnit: "minutes" | "hours";
  animation: string;
  visibleForSeconds: number;
  soundEnabled: boolean;
  soundCueId: string;
  enabled: boolean;
  nextRunAt: number;
};

type BubbleStyle = "lime" | "pink" | "yellow" | "cyan";

type PetSettings = {
  petSize: number;
  bubbleStyle: BubbleStyle;
  activeCharacterId: string;
  darkMode: boolean;
  soundEnabled: boolean;
  soundVolume: number;
};

type SoundCue = {
  id: string;
  name: string;
  isBuiltin: boolean;
  format: string;
};

type SoundCueImport = {
  name: string;
  fileName: string;
  bytes: number[];
};

const BUILTIN_SOUND_CUES: SoundCue[] = [
  {
    id: "builtin-gentle-chime",
    name: "Gentle Chime",
    isBuiltin: true,
    format: "WAV",
  },
  {
    id: "builtin-bright-pop",
    name: "Bright Pop",
    isBuiltin: true,
    format: "WAV",
  },
  {
    id: "builtin-soft-bell",
    name: "Soft Bell",
    isBuiltin: true,
    format: "WAV",
  },
];

async function playSoundCuePreview(soundCueId: string, volume: number) {
  const dataUrl = await invoke<string>("get_sound_cue_data_url", {
    id: soundCueId,
  });
  const audio = new Audio(dataUrl);
  audio.volume = Math.min(100, Math.max(0, volume)) / 100;
  await audio.play();
  return audio;
}

type CharacterAnimation = {
  id: string;
  name: string;
  kind: "idle" | "runLeft" | "runRight" | "hover" | "custom";
  frameCount: number;
};

type CharacterPack = {
  id: string;
  name: string;
  isBuiltin: boolean;
  isReady: boolean;
  previewDataUrl: string;
  animations: CharacterAnimation[];
  totalFrames: number;
};

type CharacterAnimationImportFile = {
  fileName: string;
  bytes: number[];
};

const BUBBLE_STYLES: {
  id: BubbleStyle;
  name: string;
  description: string;
}[] = [
  { id: "lime", name: "Lime Punch", description: "Bright and energetic" },
  { id: "pink", name: "Pink Pop", description: "Playful and bold" },
  { id: "yellow", name: "Sunny Note", description: "Warm and direct" },
  { id: "cyan", name: "Sky Signal", description: "Cool and crisp" },
];

const REMINDER_DURATION_OPTIONS = [
  { value: 5, label: "5 seconds" },
  { value: 10, label: "10 seconds" },
  { value: 15, label: "15 seconds" },
  { value: 30, label: "30 seconds" },
  { value: 60, label: "1 minute" },
  { value: 0, label: "Until dismissed" },
] as const;

function reminderDurationLabel(seconds: number) {
  if (seconds === 0) return "Stays until dismissed";
  if (seconds === 60) return "Visible for 1 minute";
  return `Visible for ${seconds} seconds`;
}

const CORE_ANIMATION_SLOTS = [
  { id: "idle", name: "Idle" },
  { id: "run-left", name: "Run left" },
  { id: "run-right", name: "Run right" },
  { id: "hover", name: "Hover" },
] as const;

const BUILTIN_CHARACTER: CharacterPack = {
  id: "builtin-baalert",
  name: "Baalert",
  isBuiltin: true,
  isReady: true,
  previewDataUrl: petIdle,
  animations: [
    { id: "idle", name: "Idle", kind: "idle", frameCount: 1 },
    { id: "run-left", name: "Run left", kind: "runLeft", frameCount: 1 },
    { id: "run-right", name: "Run right", kind: "runRight", frameCount: 1 },
  ],
  totalFrames: 3,
};

function nextRunLabel(reminder: Reminder) {
  if (!reminder.enabled) return "Paused";

  const remainingMs = Math.max(0, reminder.nextRunAt - Date.now());
  const remainingMinutes = Math.ceil(remainingMs / 60_000);
  if (remainingMinutes <= 1) return "Due in under a minute";
  if (remainingMinutes < 60) return `Due in ${remainingMinutes} minutes`;

  const remainingHours = Math.ceil(remainingMinutes / 60);
  return `Due in ${remainingHours} ${remainingHours === 1 ? "hour" : "hours"}`;
}

function AnimationPreview({
  name,
  frames,
  onClose,
}: {
  name: string;
  frames: string[];
  onClose: () => void;
}) {
  const [frameIndex, setFrameIndex] = useState(0);

  useEffect(() => {
    setFrameIndex(0);
    if (frames.length <= 1) return;

    const frameTimer = window.setInterval(() => {
      setFrameIndex((current) => (current + 1) % frames.length);
    }, 90);

    return () => window.clearInterval(frameTimer);
  }, [frames]);

  return (
    <div className="animation-preview-backdrop" role="presentation">
      <div
        className="animation-preview-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={`${name} animation preview`}
      >
        <div className="animation-preview-header">
          <div>
            <span>Animation preview</span>
            <strong>{name}</strong>
          </div>
          <button type="button" aria-label="Close preview" onClick={onClose}>
            x
          </button>
        </div>
        <div className="animation-preview-stage">
          <img src={frames[frameIndex]} alt="" />
        </div>
        <div className="animation-preview-progress">
          <span
            style={{
              width: `${Math.max(8, ((frameIndex + 1) / frames.length) * 100)}%`,
            }}
          />
        </div>
      </div>
    </div>
  );
}

function RemindersPage({
  onRemindersChange,
  animations,
  soundCues,
  soundVolume,
}: {
  onRemindersChange: (reminders: Reminder[]) => void;
  animations: CharacterAnimation[];
  soundCues: SoundCue[];
  soundVolume: number;
}) {
  const [reminders, setReminders] = useState<Reminder[]>([]);
  const [title, setTitle] = useState("");
  const [message, setMessage] = useState("");
  const [intervalValue, setIntervalValue] = useState(30);
  const [intervalUnit, setIntervalUnit] = useState<"minutes" | "hours">(
    "minutes",
  );
  const [animation, setAnimation] = useState("idle");
  const [visibleForSeconds, setVisibleForSeconds] = useState(10);
  const [soundEnabled, setSoundEnabled] = useState(false);
  const [soundCueId, setSoundCueId] = useState("builtin-gentle-chime");
  const [busy, setBusy] = useState(false);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [animationUpdatingId, setAnimationUpdatingId] = useState<string | null>(
    null,
  );
  const [previewingAnimation, setPreviewingAnimation] = useState<string | null>(
    null,
  );
  const [previewingSoundCue, setPreviewingSoundCue] = useState(false);
  const soundPreview = useRef<HTMLAudioElement | null>(null);
  const [animationPreview, setAnimationPreview] = useState<{
    name: string;
    frames: string[];
  } | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [error, setError] = useState("");

  function updateReminderState(
    updater: (current: Reminder[]) => Reminder[],
  ) {
    setReminders((current) => {
      const updated = updater(current);
      onRemindersChange(updated);
      return updated;
    });
  }

  async function refreshReminders() {
    try {
      const saved = await invoke<Reminder[]>("list_reminders");
      setReminders(saved);
      onRemindersChange(saved);
    } catch (reason) {
      if ("__TAURI_INTERNALS__" in window) {
        setError(String(reason));
      }
    }
  }

  useEffect(() => {
    void refreshReminders();
    const refreshTimer = window.setInterval(() => {
      void refreshReminders();
    }, 15_000);

    return () => window.clearInterval(refreshTimer);
  }, []);

  useEffect(() => {
    if (!animations.some((item) => item.id === animation)) {
      setAnimation(animations[0]?.id ?? "idle");
    }
  }, [animations, animation]);

  useEffect(() => {
    if (!soundCues.some((item) => item.id === soundCueId)) {
      setSoundCueId(soundCues[0]?.id ?? "builtin-gentle-chime");
    }
  }, [soundCues, soundCueId]);

  useEffect(
    () => () => {
      soundPreview.current?.pause();
    },
    [],
  );

  function selectUnit(unit: "minutes" | "hours") {
    setIntervalUnit(unit);
    setIntervalValue((current) => Math.min(current, unit === "minutes" ? 60 : 24));
  }

  function openCreateEditor() {
    setTitle("");
    setMessage("");
    setIntervalValue(30);
    setIntervalUnit("minutes");
    setAnimation(animations[0]?.id ?? "idle");
    setVisibleForSeconds(10);
    setSoundEnabled(false);
    setSoundCueId(soundCues[0]?.id ?? "builtin-gentle-chime");
    setEditingId(null);
    setError("");
    setEditorOpen(true);
  }

  function openEditEditor(reminder: Reminder) {
    setTitle(reminder.title);
    setMessage(reminder.message);
    setIntervalValue(reminder.intervalValue);
    setIntervalUnit(reminder.intervalUnit);
    setAnimation(reminder.animation);
    setVisibleForSeconds(reminder.visibleForSeconds);
    setSoundEnabled(reminder.soundEnabled);
    setSoundCueId(reminder.soundCueId);
    setEditingId(reminder.id);
    setError("");
    setEditorOpen(true);
  }

  function closeEditor() {
    if (busy) return;
    setEditorOpen(false);
    setEditingId(null);
    setError("");
  }

  async function handleSaveReminder(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const command = editingId ? "update_reminder" : "create_reminder";
      const reminder = await invoke<Reminder>(command, {
        ...(editingId ? { id: editingId } : {}),
        title,
        message,
        intervalValue,
        intervalUnit,
        animation,
        visibleForSeconds,
        soundEnabled,
        soundCueId,
      });
      updateReminderState((current) =>
        editingId
          ? current.map((item) => (item.id === reminder.id ? reminder : item))
          : [...current, reminder],
      );
      setEditorOpen(false);
      setEditingId(null);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function handleToggle(reminder: Reminder) {
    setError("");
    try {
      const updated = await invoke<Reminder>("set_reminder_enabled", {
        id: reminder.id,
        enabled: !reminder.enabled,
      });
      updateReminderState((current) =>
        current.map((item) => (item.id === updated.id ? updated : item)),
      );
    } catch (reason) {
      setError(String(reason));
    }
  }

  async function handleRunNow(reminder: Reminder) {
    setTestingId(reminder.id);
    setError("");
    try {
      await invoke("trigger_reminder_now", { id: reminder.id });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setTestingId(null);
    }
  }

  async function handleAnimationChange(
    reminder: Reminder,
    nextAnimation: string,
  ) {
    setAnimationUpdatingId(reminder.id);
    setError("");
    try {
      const updated = await invoke<Reminder>("set_reminder_animation", {
        id: reminder.id,
        animation: nextAnimation,
      });
      updateReminderState((current) =>
        current.map((item) => (item.id === updated.id ? updated : item)),
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setAnimationUpdatingId(null);
    }
  }

  async function handleAnimationPreview(animationId: string) {
    setPreviewingAnimation(animationId);
    setError("");
    try {
      const frames = await invoke<string[]>("get_animation_preview_frames", {
        animationId,
        characterId: null,
      });
      if (frames.length === 0) {
        throw new Error("This animation does not contain any PNG frames.");
      }
      const name =
        animations.find((item) => item.id === animationId)?.name ?? "Idle";
      setAnimationPreview({ name, frames });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPreviewingAnimation(null);
    }
  }

  async function handleSoundPreview() {
    setPreviewingSoundCue(true);
    setError("");
    try {
      soundPreview.current?.pause();
      soundPreview.current = await playSoundCuePreview(soundCueId, soundVolume);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPreviewingSoundCue(false);
    }
  }

  async function handleDelete(reminder: Reminder) {
    setError("");
    try {
      await invoke("delete_reminder", { id: reminder.id });
      updateReminderState((current) =>
        current.filter((item) => item.id !== reminder.id),
      );
    } catch (reason) {
      setError(String(reason));
    }
  }

  const maxInterval = intervalUnit === "minutes" ? 60 : 24;
  const activeCount = reminders.filter((reminder) => reminder.enabled).length;

  return (
    <div className="reminders-page">
      {animationPreview && (
        <AnimationPreview
          name={animationPreview.name}
          frames={animationPreview.frames}
          onClose={() => setAnimationPreview(null)}
        />
      )}
      <section className="reminder-list-section">
        <div className="reminder-section-heading">
          <div>
            <h2>Scheduled reminders</h2>
            <p>
              {reminders.length} total · {activeCount} active
            </p>
          </div>
          <button
            className="add-reminder-trigger"
            type="button"
            onClick={openCreateEditor}
          >
            + Add reminder
          </button>
        </div>

        {!editorOpen && error && (
          <div className="reminder-error reminder-list-error">{error}</div>
        )}

        <div className="reminder-list">
          {reminders.length === 0 && (
            <div className="reminder-empty">
              <strong>No reminders yet</strong>
              <span>Add one when you are ready.</span>
            </div>
          )}

          {reminders.map((reminder) => (
            <article
              className={`reminder-item ${reminder.enabled ? "" : "paused"}`}
              key={reminder.id}
            >
              <div className="reminder-item-header">
                <div>
                  <h3>{reminder.title}</h3>
                  <p>{reminder.message}</p>
                </div>
                <label className="compact-toggle" title="Enable reminder">
                  <input
                    type="checkbox"
                    checked={reminder.enabled}
                    onChange={() => void handleToggle(reminder)}
                  />
                  <span />
                </label>
              </div>

              <div className="reminder-animation-row">
                <span>Animation</span>
                <div className="animation-picker-control compact">
                  <select
                    aria-label={`Animation for ${reminder.title}`}
                    value={reminder.animation}
                    disabled={animationUpdatingId === reminder.id}
                    onChange={(event) =>
                      void handleAnimationChange(reminder, event.target.value)
                    }
                  >
                    {!animations.some(
                      (animationOption) =>
                        animationOption.id === reminder.animation,
                    ) && (
                      <option value={reminder.animation}>
                        Unavailable · uses Idle
                      </option>
                    )}
                    {animations.map((animationOption) => (
                      <option
                        value={animationOption.id}
                        key={animationOption.id}
                      >
                        {animationOption.name}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    disabled={previewingAnimation !== null}
                    onClick={() =>
                      void handleAnimationPreview(reminder.animation)
                    }
                  >
                    {previewingAnimation === reminder.animation
                      ? "Loading..."
                      : "Preview"}
                  </button>
                </div>
              </div>

              <div className="reminder-sound-row">
                <span>Sound cue</span>
                <strong>
                  {reminder.soundEnabled
                    ? (soundCues.find((cue) => cue.id === reminder.soundCueId)
                        ?.name ?? "Unavailable sound")
                    : "Off"}
                </strong>
              </div>

              <div className="reminder-item-footer">
                <div className="reminder-schedule">
                  <strong>
                    Every {reminder.intervalValue} {reminder.intervalUnit}
                  </strong>
                  <span>{nextRunLabel(reminder)}</span>
                  <small>{reminderDurationLabel(reminder.visibleForSeconds)}</small>
                </div>
                <div className="reminder-actions">
                  <button
                    type="button"
                    onClick={() => openEditEditor(reminder)}
                  >
                    Edit
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleRunNow(reminder)}
                    disabled={testingId === reminder.id}
                  >
                    {testingId === reminder.id ? "Showing..." : "Show now"}
                  </button>
                  <button
                    type="button"
                    className="delete-reminder-button"
                    aria-label={`Delete ${reminder.title}`}
                    title="Delete reminder"
                    onClick={() => void handleDelete(reminder)}
                  >
                    x
                  </button>
                </div>
              </div>
            </article>
          ))}
        </div>
      </section>

      {editorOpen && (
        <div className="reminder-editor-backdrop" role="presentation">
          <section
            className="reminder-editor"
            role="dialog"
            aria-modal="true"
            aria-labelledby="reminder-editor-title"
          >
            <div className="reminder-editor-header">
              <div>
                <span>{editingId ? "Edit reminder" : "New reminder"}</span>
                <h2 id="reminder-editor-title">
                  {editingId ? "Update this pet message" : "Schedule a pet message"}
                </h2>
              </div>
              <button
                type="button"
                aria-label="Close reminder editor"
                title="Close"
                onClick={closeEditor}
              >
                x
              </button>
            </div>

            <form onSubmit={handleSaveReminder}>
              <label className="reminder-field">
                <span>Title</span>
                <input
                  autoFocus
                  type="text"
                  maxLength={80}
                  value={title}
                  placeholder="Drink water"
                  onChange={(event) => setTitle(event.target.value)}
                />
              </label>

              <div className="reminder-field reminder-animation-field">
                <span>Pet animation</span>
                <div className="animation-picker-control">
                  <select
                    aria-label="Pet animation"
                    value={animation}
                    onChange={(event) => setAnimation(event.target.value)}
                  >
                    {animations.map((item) => (
                      <option value={item.id} key={item.id}>
                        {item.name} · {item.frameCount}{" "}
                        {item.frameCount === 1 ? "frame" : "frames"}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    disabled={previewingAnimation !== null}
                    onClick={() => void handleAnimationPreview(animation)}
                  >
                    {previewingAnimation === animation
                      ? "Loading..."
                      : "Preview"}
                  </button>
                </div>
              </div>

              <label className="reminder-field">
                <span>Notification message</span>
                <textarea
                  maxLength={MAX_REMINDER_MESSAGE_LENGTH}
                  value={message}
                  placeholder="Time to take a short water break."
                  onChange={(event) => setMessage(event.target.value)}
                />
                <small className="reminder-character-count">
                  {message.length}/{MAX_REMINDER_MESSAGE_LENGTH} characters · Bubble
                  expands automatically
                </small>
              </label>

              <div className="reminder-frequency">
                <label className="reminder-field interval-field">
                  <span>Every</span>
                  <input
                    type="number"
                    min={1}
                    max={maxInterval}
                    value={intervalValue}
                    onChange={(event) =>
                      setIntervalValue(
                        Math.max(
                          1,
                          Math.min(maxInterval, Number(event.target.value)),
                        ),
                      )
                    }
                  />
                </label>

                <div className="unit-control" aria-label="Interval unit">
                  <button
                    type="button"
                    className={intervalUnit === "minutes" ? "active" : ""}
                    onClick={() => selectUnit("minutes")}
                  >
                    Minutes
                  </button>
                  <button
                    type="button"
                    className={intervalUnit === "hours" ? "active" : ""}
                    onClick={() => selectUnit("hours")}
                  >
                    Hours
                  </button>
                </div>
              </div>

              <label className="reminder-field">
                <span>Bubble duration</span>
                <select
                  value={visibleForSeconds}
                  onChange={(event) =>
                    setVisibleForSeconds(Number(event.target.value))
                  }
                >
                  {REMINDER_DURATION_OPTIONS.map((option) => (
                    <option value={option.value} key={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>

              <div className="reminder-field">
                <span>Sound cue</span>
                <div className="reminder-sound-picker">
                  <label
                    className="compact-toggle"
                    title={soundEnabled ? "Disable sound cue" : "Enable sound cue"}
                  >
                    <input
                      type="checkbox"
                      checked={soundEnabled}
                      onChange={(event) => setSoundEnabled(event.target.checked)}
                    />
                    <span />
                  </label>
                  <select
                    aria-label="Reminder sound cue"
                    value={soundCueId}
                    disabled={!soundEnabled}
                    onChange={(event) => setSoundCueId(event.target.value)}
                  >
                    {soundCues.map((cue) => (
                      <option value={cue.id} key={cue.id}>
                        {cue.name}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    disabled={!soundEnabled || previewingSoundCue}
                    onClick={() => void handleSoundPreview()}
                  >
                    {previewingSoundCue ? "Playing..." : "Preview"}
                  </button>
                </div>
              </div>

              {error && <div className="reminder-error">{error}</div>}

              <div className="reminder-editor-actions">
                <button type="button" onClick={closeEditor}>
                  Cancel
                </button>
                <button
                  className="create-reminder-button"
                  type="submit"
                  disabled={busy || !title.trim() || !message.trim()}
                >
                  {busy
                    ? editingId
                      ? "Saving..."
                      : "Adding..."
                    : editingId
                      ? "Save changes"
                      : "Add reminder"}
                </button>
              </div>
            </form>
          </section>
        </div>
      )}
    </div>
  );
}

type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "latest"
  | "downloading"
  | "restarting"
  | "error";

function AppUpdateTool() {
  const [currentVersion, setCurrentVersion] = useState("0.1.0");
  const [availableUpdate, setAvailableUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState("");

  useEffect(() => {
    void getVersion().then(setCurrentVersion).catch(() => undefined);
  }, []);

  async function handleCheckForUpdates() {
    setStatus("checking");
    setError("");
    setProgress(0);

    try {
      const update = await check({ timeout: 20_000 });
      setAvailableUpdate(update);
      setStatus(update ? "available" : "latest");
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(
        message.includes("404")
          ? "No published Baalert release is available yet."
          : message,
      );
      setStatus("error");
    }
  }

  async function handleInstallUpdate() {
    if (!availableUpdate) return;

    setStatus("downloading");
    setError("");
    setProgress(0);
    let downloaded = 0;
    let total = 0;

    try {
      await availableUpdate.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          if (total > 0) {
            setProgress(Math.min(100, Math.round((downloaded / total) * 100)));
          }
        } else {
          setProgress(100);
        }
      });
      setStatus("restarting");
      await relaunch();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setStatus("error");
    }
  }

  const busy = status === "checking" || status === "downloading" || status === "restarting";

  return (
    <section className="app-update-tool">
      <div className="app-update-heading">
        <div>
          <h2>App updates</h2>
          <p>Current version v{currentVersion}</p>
        </div>
        <span className={`update-state ${status}`}>
          {status === "checking" && "Checking"}
          {status === "available" && `v${availableUpdate?.version} ready`}
          {status === "latest" && "Up to date"}
          {status === "downloading" && `${progress}%`}
          {status === "restarting" && "Restarting"}
          {status === "error" && "Check failed"}
          {status === "idle" && "Stable channel"}
        </span>
      </div>

      {availableUpdate && status === "available" && (
        <div className="update-release">
          <strong>Baalert v{availableUpdate.version}</strong>
          <p>{availableUpdate.body?.trim() || "A new version is ready to install."}</p>
        </div>
      )}

      {status === "downloading" && (
        <div
          className="update-progress"
          role="progressbar"
          aria-label="Downloading update"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={progress}
        >
          <span style={{ width: `${Math.max(3, progress)}%` }} />
        </div>
      )}

      {error && <div className="update-error">{error}</div>}

      <div className="app-update-actions">
        <button
          className="release-history-button"
          type="button"
          onClick={() =>
            void openUrl(
              "https://github.com/Dewakd/baalert-reminder-app/releases",
            )
          }
        >
          Release history
        </button>
        {availableUpdate && status === "available" ? (
          <button type="button" onClick={() => void handleInstallUpdate()}>
            Download and restart
          </button>
        ) : (
          <button
            type="button"
            disabled={busy}
            onClick={() => void handleCheckForUpdates()}
          >
            {status === "checking" ? "Checking..." : "Check for updates"}
          </button>
        )}
      </div>
    </section>
  );
}

function SettingsPage({
  petSize,
  bubbleStyle,
  soundEnabled,
  soundVolume,
  soundCues,
  soundBusy,
  soundError,
  characters,
  activeCharacterId,
  characterBusy,
  characterError,
  onPetSizePreview,
  onPetSizeCommit,
  onBubbleStyleChange,
  onSoundSettingsChange,
  onSoundImport,
  onSoundDelete,
  onCharacterCreate,
  onCharacterAnimationAdd,
  onCharacterAnimationDelete,
  onCharacterSelect,
  onCharacterDelete,
}: {
  petSize: number;
  bubbleStyle: BubbleStyle;
  soundEnabled: boolean;
  soundVolume: number;
  soundCues: SoundCue[];
  soundBusy: boolean;
  soundError: string;
  characters: CharacterPack[];
  activeCharacterId: string;
  characterBusy: boolean;
  characterError: string;
  onPetSizePreview: (size: number) => void;
  onPetSizeCommit: (size: number) => void;
  onBubbleStyleChange: (style: BubbleStyle) => void;
  onSoundSettingsChange: (enabled: boolean, volume: number) => Promise<void>;
  onSoundImport: (sound: SoundCueImport) => Promise<void>;
  onSoundDelete: (soundCueId: string) => Promise<void>;
  onCharacterCreate: (name: string) => Promise<void>;
  onCharacterAnimationAdd: (
    characterId: string,
    animation: string,
    files: CharacterAnimationImportFile[],
  ) => Promise<void>;
  onCharacterAnimationDelete: (
    characterId: string,
    animationId: string,
  ) => Promise<void>;
  onCharacterSelect: (characterId: string) => Promise<void>;
  onCharacterDelete: (characterId: string) => Promise<void>;
}) {
  const previewSize = Math.round(petSize * 0.72);
  const animationInputRef = useRef<HTMLInputElement>(null);
  const soundInputRef = useRef<HTMLInputElement>(null);
  const soundPreview = useRef<HTMLAudioElement | null>(null);
  const [animationUploadTarget, setAnimationUploadTarget] = useState<{
    characterId: string;
    animation: string;
  } | null>(null);
  const [newCharacterOpen, setNewCharacterOpen] = useState(false);
  const [tutorialOpen, setTutorialOpen] = useState(false);
  const [customAnimationCharacterId, setCustomAnimationCharacterId] = useState<
    string | null
  >(null);
  const [newCharacterName, setNewCharacterName] = useState("");
  const [customAnimationName, setCustomAnimationName] = useState("");
  const [localImportError, setLocalImportError] = useState("");
  const [previewingCharacterAnimation, setPreviewingCharacterAnimation] =
    useState<string | null>(null);
  const [previewingSoundCueId, setPreviewingSoundCueId] = useState<string | null>(
    null,
  );
  const [localSoundError, setLocalSoundError] = useState("");
  const [draftSoundVolume, setDraftSoundVolume] = useState(soundVolume);
  const [characterAnimationPreview, setCharacterAnimationPreview] = useState<{
    name: string;
    frames: string[];
  } | null>(null);
  const activeCharacter =
    characters.find((character) => character.id === activeCharacterId) ??
    characters[0];

  useEffect(
    () => () => {
      soundPreview.current?.pause();
    },
    [],
  );

  useEffect(() => setDraftSoundVolume(soundVolume), [soundVolume]);

  async function handleSoundPreview(soundCueId: string) {
    setPreviewingSoundCueId(soundCueId);
    setLocalSoundError("");
    try {
      soundPreview.current?.pause();
      soundPreview.current = await playSoundCuePreview(
        soundCueId,
        draftSoundVolume,
      );
    } catch (reason) {
      setLocalSoundError(String(reason));
    } finally {
      setPreviewingSoundCueId(null);
    }
  }

  async function handleSoundFile(event: React.ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;

    setLocalSoundError("");
    try {
      const name = file.name.replace(/\.[^.]+$/, "").trim() || "Custom cue";
      await onSoundImport({
        name,
        fileName: file.name,
        bytes: Array.from(new Uint8Array(await file.arrayBuffer())),
      });
    } catch (reason) {
      setLocalSoundError(String(reason));
    }
  }

  async function handleCreateCharacter(event: React.FormEvent) {
    event.preventDefault();
    const name = newCharacterName.trim();
    setLocalImportError("");

    try {
      if (!name) throw new Error("Enter a name for the character.");
      await onCharacterCreate(name);
      setNewCharacterName("");
      setNewCharacterOpen(false);
    } catch (reason) {
      setLocalImportError(
        reason instanceof Error ? reason.message : String(reason),
      );
    }
  }

  function openAnimationFolder(characterId: string, animation: string) {
    setAnimationUploadTarget({ characterId, animation });
    setLocalImportError("");
    window.setTimeout(() => animationInputRef.current?.click(), 0);
  }

  function handleCustomAnimationSubmit(event: React.FormEvent) {
    event.preventDefault();
    const name = customAnimationName.trim();
    if (!customAnimationCharacterId || !name) {
      setLocalImportError("Enter a name for the custom animation.");
      return;
    }

    const characterId = customAnimationCharacterId;
    setCustomAnimationCharacterId(null);
    setCustomAnimationName("");
    openAnimationFolder(characterId, `custom/${name}`);
  }

  async function handleAnimationFolder(
    event: React.ChangeEvent<HTMLInputElement>,
  ) {
    const input = event.currentTarget;
    const target = animationUploadTarget;
    const selectedFiles = Array.from(input.files ?? [])
      .filter((file) => file.name.toLowerCase().endsWith(".png"))
      .sort((left, right) =>
        left.webkitRelativePath.localeCompare(right.webkitRelativePath),
      );
    setLocalImportError("");

    try {
      if (!target) throw new Error("Choose an animation slot first.");
      if (selectedFiles.length === 0) {
        throw new Error("The selected animation folder has no PNG frames.");
      }
      if (selectedFiles.length > 500) {
        throw new Error("A character can contain up to 500 PNG frames.");
      }
      const totalBytes = selectedFiles.reduce(
        (total, file) => total + file.size,
        0,
      );
      if (totalBytes > 40 * 1024 * 1024) {
        throw new Error("Character files must stay under 40 MB.");
      }

      const importFiles = await Promise.all(
        selectedFiles.map(async (file) => ({
          fileName: file.name,
          bytes: Array.from(new Uint8Array(await file.arrayBuffer())),
        })),
      );
      await onCharacterAnimationAdd(
        target.characterId,
        target.animation,
        importFiles,
      );
    } catch (reason) {
      setLocalImportError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      input.value = "";
      setAnimationUploadTarget(null);
    }
  }

  async function handleCharacterAnimationDelete(
    character: CharacterPack,
    animation: CharacterAnimation,
  ) {
    if (
      !window.confirm(
        `Delete ${animation.name} from ${character.name}?`,
      )
    ) {
      return;
    }
    setLocalImportError("");
    try {
      await onCharacterAnimationDelete(character.id, animation.id);
    } catch (reason) {
      setLocalImportError(
        reason instanceof Error ? reason.message : String(reason),
      );
    }
  }

  async function handleCharacterAnimationPreview(
    character: CharacterPack,
    animation: CharacterAnimation,
  ) {
    const previewKey = `${character.id}:${animation.id}`;
    setPreviewingCharacterAnimation(previewKey);
    setLocalImportError("");

    try {
      const frames = await invoke<string[]>("get_animation_preview_frames", {
        characterId: character.id,
        animationId: animation.id,
      });
      if (frames.length === 0) {
        throw new Error("This animation does not contain any PNG frames.");
      }
      setCharacterAnimationPreview({
        name: `${character.name} · ${animation.name}`,
        frames,
      });
    } catch (reason) {
      setLocalImportError(
        reason instanceof Error ? reason.message : String(reason),
      );
    } finally {
      setPreviewingCharacterAnimation(null);
    }
  }

  return (
    <div className="settings-page">
      {characterAnimationPreview && (
        <AnimationPreview
          name={characterAnimationPreview.name}
          frames={characterAnimationPreview.frames}
          onClose={() => setCharacterAnimationPreview(null)}
        />
      )}
      <section className="pet-size-tool">
        <div className="reminder-section-heading">
          <div>
            <h2>Pet size</h2>
            <p>Saved size is used by the desktop pet and reminder alerts.</p>
          </div>
          <span>{petSize}px</span>
        </div>

        <div className="pet-size-preview">
          <img
            src={activeCharacter?.previewDataUrl || petIdle}
            alt="Pet size preview"
            style={{ width: previewSize, height: previewSize }}
          />
        </div>

        <div className="pet-size-slider-header">
          <span>Small</span>
          <span>Large</span>
        </div>
        <input
          className="pet-size-slider"
          type="range"
          min={96}
          max={224}
          step={8}
          value={petSize}
          onChange={(event) => onPetSizePreview(Number(event.target.value))}
          onPointerUp={(event) =>
            onPetSizeCommit(Number(event.currentTarget.value))
          }
          onKeyUp={(event) =>
            onPetSizeCommit(Number(event.currentTarget.value))
          }
          onBlur={(event) => onPetSizeCommit(Number(event.currentTarget.value))}
        />

        <div className="pet-size-presets">
          {[
            { label: "Small", value: 112 },
            { label: "Default", value: 152 },
            { label: "Large", value: 200 },
          ].map((preset) => (
            <button
              key={preset.value}
              type="button"
              className={petSize === preset.value ? "active" : ""}
              onClick={() => {
                onPetSizePreview(preset.value);
                onPetSizeCommit(preset.value);
              }}
            >
              {preset.label}
            </button>
          ))}
        </div>
      </section>

      <section className="bubble-style-tool">
        <div className="reminder-section-heading">
          <div>
            <h2>Chat bubble style</h2>
            <p>Choose the color used when your pet delivers a reminder.</p>
          </div>
          <span className={`current-style-chip ${bubbleStyle}`}>
            {BUBBLE_STYLES.find((style) => style.id === bubbleStyle)?.name}
          </span>
        </div>

        <div className="bubble-style-preview">
          <img
            src={activeCharacter?.previewDataUrl || petIdle}
            alt="Pet with reminder preview"
          />
          <div className={`bubble-theme-preview ${bubbleStyle}`}>
            <button type="button" tabIndex={-1} aria-label="Close preview">
              x
            </button>
            <strong>Stretch break</strong>
            <span>Stand up and move around for a minute.</span>
          </div>
        </div>

        <div className="bubble-style-options" aria-label="Chat bubble style">
          {BUBBLE_STYLES.map((style) => (
            <button
              type="button"
              key={style.id}
              className={`bubble-style-option ${style.id} ${
                bubbleStyle === style.id ? "active" : ""
              }`}
              aria-pressed={bubbleStyle === style.id}
              onClick={() => onBubbleStyleChange(style.id)}
            >
              <span className="bubble-style-swatch" />
              <span>
                <strong>{style.name}</strong>
                <small>{style.description}</small>
              </span>
              <b>{bubbleStyle === style.id ? "Selected" : "Choose"}</b>
            </button>
          ))}
        </div>
      </section>

      <section className="sound-settings-tool">
        <div className="reminder-section-heading sound-settings-heading">
          <div>
            <h2>Sound cues</h2>
            <p>Play a cue with reminders that have sound enabled.</p>
          </div>
          <div className="toggle-wrapper">
            <span className={`toggle-label ${soundEnabled ? "on" : ""}`}>
              {soundEnabled ? "ON" : "OFF"}
            </span>
            <label className="toggle" title="Toggle all reminder sounds">
              <input
                type="checkbox"
                checked={soundEnabled}
                onChange={(event) =>
                  void onSoundSettingsChange(
                    event.target.checked,
                    draftSoundVolume,
                  )
                }
              />
              <span className="toggle-track" />
              <span className="toggle-thumb" />
            </label>
          </div>
        </div>

        <div className="sound-volume-control">
          <div>
            <strong>Volume</strong>
            <span>{draftSoundVolume}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={100}
            step={5}
            value={draftSoundVolume}
            disabled={!soundEnabled}
            aria-label="Sound cue volume"
            onChange={(event) => setDraftSoundVolume(Number(event.target.value))}
            onPointerUp={(event) =>
              void onSoundSettingsChange(
                soundEnabled,
                Number(event.currentTarget.value),
              )
            }
            onKeyUp={(event) =>
              void onSoundSettingsChange(
                soundEnabled,
                Number(event.currentTarget.value),
              )
            }
            onBlur={(event) =>
              void onSoundSettingsChange(
                soundEnabled,
                Number(event.currentTarget.value),
              )
            }
          />
        </div>

        <div className="sound-library-heading">
          <div>
            <strong>Sound library</strong>
            <span>{soundCues.length} cues available</span>
          </div>
          <button
            type="button"
            disabled={soundBusy}
            onClick={() => soundInputRef.current?.click()}
          >
            {soundBusy ? "Importing..." : "+ Add local sound"}
          </button>
          <input
            ref={soundInputRef}
            type="file"
            accept=".mp3,.wav,audio/mpeg,audio/wav"
            hidden
            onChange={(event) => void handleSoundFile(event)}
          />
        </div>

        {(localSoundError || soundError) && (
          <div className="reminder-error">{localSoundError || soundError}</div>
        )}

        <div className="sound-cue-grid">
          {soundCues.map((cue) => (
            <article className="sound-cue-item" key={cue.id}>
              <div className="sound-cue-symbol" aria-hidden="true">
                {cue.isBuiltin ? "♪" : "+"}
              </div>
              <div>
                <strong>{cue.name}</strong>
                <span>
                  {cue.isBuiltin ? "Built in" : "Local"} · {cue.format}
                </span>
              </div>
              <button
                type="button"
                disabled={previewingSoundCueId !== null || soundVolume === 0}
                onClick={() => void handleSoundPreview(cue.id)}
              >
                {previewingSoundCueId === cue.id ? "Playing..." : "Preview"}
              </button>
              {!cue.isBuiltin && (
                <button
                  className="delete-sound-cue"
                  type="button"
                  title={`Delete ${cue.name}`}
                  aria-label={`Delete ${cue.name}`}
                  disabled={soundBusy}
                  onClick={() => {
                    if (window.confirm(`Delete ${cue.name}?`)) {
                      void onSoundDelete(cue.id);
                    }
                  }}
                >
                  x
                </button>
              )}
            </article>
          ))}
        </div>
      </section>

      <AppUpdateTool />

      <section className="character-library-tool">
        <div className="character-library-heading">
          <div>
            <h2>Character library</h2>
            <p>{characters.length} characters stored on this device</p>
          </div>
          <div className="character-library-actions">
            <button
              className="character-tutorial-button"
              type="button"
              onClick={() => setTutorialOpen(true)}
            >
              <span aria-hidden="true">?</span>
              Tutorial
            </button>
            <button
              className="add-character-button"
              type="button"
              disabled={characterBusy}
              onClick={() => {
                setLocalImportError("");
                setNewCharacterOpen(true);
              }}
            >
              {characterBusy ? "Working..." : "+ Add local character"}
            </button>
          </div>
          <input
            ref={(input) => {
              animationInputRef.current = input;
              input?.setAttribute("webkitdirectory", "");
            }}
            className="character-folder-input"
            type="file"
            accept=".png,image/png"
            multiple
            hidden
            onChange={(event) => void handleAnimationFolder(event)}
          />
        </div>

        <div className="animation-requirements">
          <span>Idle slot</span>
          <span>Run left slot</span>
          <span>Run right slot</span>
          <span>Hover slot</span>
          <span>All optional</span>
        </div>

        {(localImportError || characterError) && (
          <div className="character-error">
            {localImportError || characterError}
          </div>
        )}

        <div className="character-grid">
          {characters.map((character) => {
            const active = character.id === activeCharacterId;
            const customAnimations = character.animations.filter(
              (animation) => animation.kind === "custom",
            );
            const coreAnimations = CORE_ANIMATION_SLOTS.map((slot) => ({
              ...slot,
              animation: character.animations.find(
                (animation) => animation.id === slot.id,
              ),
            }));
            return (
              <article
                className={`character-card ${active ? "active" : ""} ${
                  character.isReady ? "ready" : "draft"
                }`}
                key={character.id}
              >
                <div className="character-art">
                  {character.previewDataUrl ? (
                    <img src={character.previewDataUrl} alt={character.name} />
                  ) : (
                    <div className="character-art-placeholder">
                      <b>?</b>
                      <span>Add animation</span>
                    </div>
                  )}
                  <span>
                    {active
                      ? "Active"
                      : character.isBuiltin
                        ? "Built in"
                        : character.isReady
                          ? "Ready"
                          : "Empty"}
                  </span>
                </div>
                <div className="character-card-body">
                  <div className="character-card-title">
                    <div>
                      <h3>{character.name}</h3>
                      <p>{character.totalFrames} animation frames</p>
                    </div>
                    {!character.isBuiltin && (
                      <button
                        className="delete-character-button"
                        type="button"
                        title={`Delete ${character.name}`}
                        aria-label={`Delete ${character.name}`}
                        disabled={characterBusy}
                        onClick={() => void onCharacterDelete(character.id)}
                      >
                        x
                      </button>
                    )}
                  </div>
                  <div className="character-core-animation-grid">
                    {coreAnimations.map(({ id, name, animation }) => {
                      const previewKey = `${character.id}:${id}`;
                      return (
                        <div
                          className={`character-animation-slot ${
                            animation ? "filled" : "empty"
                          }`}
                          key={`${character.id}-${id}`}
                        >
                          <button
                            className="character-slot-input"
                            type="button"
                            disabled={character.isBuiltin || characterBusy}
                            title={
                              character.isBuiltin
                                ? undefined
                                : `${animation ? "Replace" : "Add"} ${name} animation`
                            }
                            onClick={() =>
                              openAnimationFolder(character.id, id)
                            }
                          >
                            <span>{name}</span>
                            <small>
                              {animation
                                ? `${animation.frameCount} ${
                                    animation.frameCount === 1
                                      ? "frame"
                                      : "frames"
                                  }`
                                : character.isBuiltin
                                  ? "Not included"
                                  : "Add folder"}
                            </small>
                          </button>
                          <div className="character-slot-actions">
                            {animation && (
                              <button
                                className="preview-animation-button"
                                type="button"
                                title={`Preview ${name}`}
                                aria-label={`Preview ${name} animation for ${character.name}`}
                                disabled={
                                  previewingCharacterAnimation !== null
                                }
                                onClick={() =>
                                  void handleCharacterAnimationPreview(
                                    character,
                                    animation,
                                  )
                                }
                              >
                                {previewingCharacterAnimation === previewKey
                                  ? "..."
                                  : "▶"}
                              </button>
                            )}
                            {!character.isBuiltin && animation && (
                              <button
                                className="delete-animation-button"
                                type="button"
                                title={`Delete ${name}`}
                                aria-label={`Delete ${name} animation from ${character.name}`}
                                disabled={characterBusy}
                                onClick={() =>
                                  void handleCharacterAnimationDelete(
                                    character,
                                    animation,
                                  )
                                }
                              >
                                x
                              </button>
                            )}
                          </div>
                        </div>
                      );
                    })}
                  </div>

                  {customAnimations.length > 0 && (
                    <div className="character-custom-animation-list">
                      {customAnimations.map((animation) => {
                        const previewKey = `${character.id}:${animation.id}`;
                        return (
                          <div
                            className="character-custom-animation"
                            key={`${character.id}-${animation.id}`}
                          >
                            <button
                              className="character-custom-animation-name"
                              type="button"
                              disabled={character.isBuiltin || characterBusy}
                              title={
                                character.isBuiltin
                                  ? undefined
                                  : `Replace ${animation.name} animation`
                              }
                              onClick={() =>
                                openAnimationFolder(
                                  character.id,
                                  animation.id,
                                )
                              }
                            >
                              <span>{animation.name}</span>
                              <small>{animation.frameCount} frames</small>
                            </button>
                            <button
                              className="preview-animation-button"
                              type="button"
                              title={`Preview ${animation.name}`}
                              aria-label={`Preview ${animation.name} animation for ${character.name}`}
                              disabled={previewingCharacterAnimation !== null}
                              onClick={() =>
                                void handleCharacterAnimationPreview(
                                  character,
                                  animation,
                                )
                              }
                            >
                              {previewingCharacterAnimation === previewKey
                                ? "..."
                                : "▶"}
                            </button>
                            {!character.isBuiltin && (
                              <button
                                className="delete-animation-button"
                                type="button"
                                title={`Delete ${animation.name}`}
                                aria-label={`Delete ${animation.name} animation from ${character.name}`}
                                disabled={characterBusy}
                                onClick={() =>
                                  void handleCharacterAnimationDelete(
                                    character,
                                    animation,
                                  )
                                }
                              >
                                x
                              </button>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                  <div className="character-card-footer">
                    <span>
                      {character.isReady
                        ? `${character.animations.length} ${
                            character.animations.length === 1
                              ? "animation"
                              : "animations"
                          } available`
                        : "Add any animation to use"}
                    </span>
                    <div className="character-card-actions">
                      {!character.isBuiltin && (
                        <button
                          className="add-animation-button"
                          type="button"
                          disabled={characterBusy}
                          onClick={() => {
                            setLocalImportError("");
                            setCustomAnimationCharacterId(character.id);
                          }}
                        >
                          + Custom animation
                        </button>
                      )}
                      <button
                        type="button"
                        disabled={
                          active || characterBusy || !character.isReady
                        }
                        title={
                          character.isReady
                            ? undefined
                            : "Add at least one animation first"
                        }
                        onClick={() => void onCharacterSelect(character.id)}
                      >
                        {active
                          ? "In use"
                          : character.isReady
                            ? "Use character"
                            : "Not ready"}
                      </button>
                    </div>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      </section>

      {newCharacterOpen && (
        <div className="character-dialog-backdrop" role="presentation">
          <form
            className="character-dialog new-character-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="new-character-title"
            onSubmit={(event) => void handleCreateCharacter(event)}
          >
            <div className="character-dialog-header">
              <div>
                <span>New local character</span>
                <h2 id="new-character-title">Create the card first</h2>
              </div>
              <button
                type="button"
                aria-label="Close new character dialog"
                title="Close"
                onClick={() => setNewCharacterOpen(false)}
              >
                x
              </button>
            </div>
            <label className="character-name-field">
              <span>Character name</span>
              <input
                autoFocus
                maxLength={48}
                placeholder="Pixel Buddy"
                value={newCharacterName}
                onChange={(event) => setNewCharacterName(event.target.value)}
              />
            </label>
            <div className="character-dialog-note">
              The card appears immediately. Add animation folders from the new
              card afterward.
            </div>
            <div className="character-dialog-footer">
              <button type="button" onClick={() => setNewCharacterOpen(false)}>
                Cancel
              </button>
              <button type="submit" disabled={characterBusy}>
                {characterBusy ? "Creating..." : "Create character"}
              </button>
            </div>
          </form>
        </div>
      )}

      {customAnimationCharacterId && (
        <div className="character-dialog-backdrop" role="presentation">
          <form
            className="character-dialog new-character-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="new-animation-title"
            onSubmit={handleCustomAnimationSubmit}
          >
            <div className="character-dialog-header custom-animation-header">
              <div>
                <span>Custom reminder animation</span>
                <h2 id="new-animation-title">Name the animation</h2>
              </div>
              <button
                type="button"
                aria-label="Close custom animation dialog"
                title="Close"
                onClick={() => setCustomAnimationCharacterId(null)}
              >
                x
              </button>
            </div>
            <label className="character-name-field">
              <span>Animation name</span>
              <input
                autoFocus
                maxLength={48}
                placeholder="Wave"
                value={customAnimationName}
                onChange={(event) => setCustomAnimationName(event.target.value)}
              />
            </label>
            <div className="character-dialog-note">
              This name appears in the animation menu when creating a reminder.
            </div>
            <div className="character-dialog-footer">
              <button
                type="button"
                onClick={() => setCustomAnimationCharacterId(null)}
              >
                Cancel
              </button>
              <button type="submit" disabled={!customAnimationName.trim()}>
                Select PNG folder
              </button>
            </div>
          </form>
        </div>
      )}

      {tutorialOpen && (
        <div className="character-dialog-backdrop" role="presentation">
          <section
            className="character-dialog character-tutorial-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="character-tutorial-title"
          >
            <div className="character-dialog-header">
              <div>
                <span>Local character tutorial</span>
                <h2 id="character-tutorial-title">Prepare your PNG folders</h2>
              </div>
              <button
                type="button"
                aria-label="Close character tutorial"
                title="Close"
                onClick={() => setTutorialOpen(false)}
              >
                x
              </button>
            </div>

            <div className="tutorial-layout">
              <div className="tutorial-folder-tree">
                <strong>Recommended files</strong>
                <pre>{`Pixel Buddy/
  idle/
    001.png
    002.png
  run-left/
    001.png
    002.png
  run-right/
    001.png
    002.png
  hover/
    001.png
    002.png
  wave/
    001.png
    002.png`}</pre>
              </div>
              <ol className="tutorial-steps">
                <li>
                  <b>1</b>
                  <span>
                    Click <strong>Add local character</strong> and name it.
                  </span>
                </li>
                <li>
                  <b>2</b>
                  <span>
                    Click <strong>Idle</strong>, <strong>Run left</strong>,{" "}
                    <strong>Run right</strong>, or <strong>Hover</strong>, then
                    select the PNG folder for that behavior.
                  </span>
                </li>
                <li>
                  <b>3</b>
                  <span>
                    Every slot is optional. After one animation is added, the
                    character can be used and empty slots fall back safely.
                  </span>
                </li>
                <li>
                  <b>4</b>
                  <span>
                    Use <strong>+ Custom animation</strong> for reminder-only
                    actions. Existing animations can be previewed, replaced, or
                    deleted from the card.
                  </span>
                </li>
              </ol>
            </div>

            <div className="tutorial-limits">
              <span>PNG only</span>
              <span>500 frames maximum</span>
              <span>40 MB per character</span>
            </div>
            <div className="character-dialog-footer">
              <button type="button" onClick={() => setTutorialOpen(false)}>
                Got it
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

function Dashboard() {
  const [activePage, setActivePage] = useState<NavPage>("dashboard");
  const [petActive, setPetActive] = useState(false);
  const [petSize, setPetSize] = useState(152);
  const [bubbleStyle, setBubbleStyle] = useState<BubbleStyle>("lime");
  const [darkMode, setDarkMode] = useState(false);
  const [soundEnabled, setSoundEnabled] = useState(true);
  const [soundVolume, setSoundVolume] = useState(70);
  const [soundCues, setSoundCues] = useState<SoundCue[]>(BUILTIN_SOUND_CUES);
  const [soundBusy, setSoundBusy] = useState(false);
  const [soundError, setSoundError] = useState("");
  const [activeCharacterId, setActiveCharacterId] = useState(
    "builtin-baalert",
  );
  const [characters, setCharacters] = useState<CharacterPack[]>([
    BUILTIN_CHARACTER,
  ]);
  const [characterBusy, setCharacterBusy] = useState(false);
  const [characterError, setCharacterError] = useState("");
  const [dashboardReminders, setDashboardReminders] = useState<Reminder[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    document.documentElement.dataset.theme = darkMode ? "dark" : "light";
  }, [darkMode]);

  useEffect(() => {
    async function loadDashboardState() {
      try {
        const [settings, reminders, petVisible, savedCharacters, savedSoundCues] =
          await Promise.all([
            invoke<PetSettings>("get_pet_settings"),
            invoke<Reminder[]>("list_reminders"),
            invoke<boolean>("is_pet_visible"),
            invoke<CharacterPack[]>("list_characters"),
            invoke<SoundCue[]>("list_sound_cues"),
          ]);
        setPetSize(settings.petSize);
        setBubbleStyle(settings.bubbleStyle);
        setDarkMode(settings.darkMode);
        setSoundEnabled(settings.soundEnabled);
        setSoundVolume(settings.soundVolume);
        setSoundCues(savedSoundCues);
        setActiveCharacterId(settings.activeCharacterId);
        setCharacters(savedCharacters);
        setDashboardReminders(reminders);
        setPetActive(petVisible);
      } catch (error) {
        console.error("Dashboard state error:", error);
      }
    }

    void loadDashboardState();
    const refreshTimer = window.setInterval(() => {
      void loadDashboardState();
    }, 15_000);
    return () => window.clearInterval(refreshTimer);
  }, []);

  const activeReminderCount = dashboardReminders.filter(
    (reminder) => reminder.enabled,
  ).length;
  const activeCharacter =
    characters.find((character) => character.id === activeCharacterId) ??
    characters[0];

  const navItems: {
    id: NavPage;
    icon: string;
    label: string;
    badge?: string;
    soon?: boolean;
  }[] = [
    { id: "dashboard", icon: "⚡", label: "Dashboard" },
    { id: "calendar", icon: "📅", label: "Calendar", soon: true },
    {
      id: "reminders",
      icon: "🔔",
      label: "Reminders",
      badge: activeReminderCount ? String(activeReminderCount) : undefined,
    },
    { id: "settings", icon: "⚙️", label: "Settings" },
  ];

  const placeholderEvents = [
    { color: "#8b5cf6", title: "Team Standup", time: "09:00 AM", tag: "in 2h" },
    { color: "#34d399", title: "Design Review", time: "11:30 AM", tag: "in 4h" },
    { color: "#fbbf24", title: "Lunch with Alex", time: "01:00 PM", tag: "in 6h" },
    { color: "#f87171", title: "Sprint Planning", time: "03:00 PM", tag: "in 8h" },
  ];

  async function handlePetToggle(enabled: boolean) {
    setLoading(true);
    try {
      if (enabled) {
        await invoke("show_pet", {
          title: null,
          message: "",
          showAfterSeconds: 0,
          visibleForSeconds: 2,
          showBubble: false,
          petSize,
          bubbleStyle,
          reminderAnimation: null,
          soundCueId: null,
          soundVolume: null,
        });
      } else {
        await invoke("hide_pet");
      }
      setPetActive(enabled);
    } catch (err) {
      console.error("Pet toggle error:", err);
    } finally {
      setLoading(false);
    }
  }

  async function handlePetSizeCommit(size: number) {
    try {
      const settings = await invoke<PetSettings>("set_pet_size", {
        petSize: size,
      });
      setPetSize(settings.petSize);
      if (petActive) {
        await invoke("show_pet", {
          title: null,
          message: "",
          showAfterSeconds: 0,
          visibleForSeconds: 2,
          showBubble: false,
          petSize: settings.petSize,
          bubbleStyle,
          reminderAnimation: null,
          soundCueId: null,
          soundVolume: null,
        });
      }
    } catch (err) {
      console.error("Pet size update error:", err);
    }
  }

  async function handleBubbleStyleChange(style: BubbleStyle) {
    setBubbleStyle(style);
    try {
      const settings = await invoke<PetSettings>("set_bubble_style", {
        bubbleStyle: style,
      });
      setBubbleStyle(settings.bubbleStyle);
    } catch (err) {
      console.error("Bubble style update error:", err);
    }
  }

  async function handleDarkModeToggle() {
    const nextDarkMode = !darkMode;
    setDarkMode(nextDarkMode);
    try {
      const settings = await invoke<PetSettings>("set_dark_mode", {
        darkMode: nextDarkMode,
      });
      setDarkMode(settings.darkMode);
    } catch (err) {
      console.error("Theme update error:", err);
    }
  }

  async function handleSoundSettingsChange(enabled: boolean, volume: number) {
    setSoundEnabled(enabled);
    setSoundVolume(volume);
    setSoundError("");
    try {
      const settings = await invoke<PetSettings>("set_sound_settings", {
        soundEnabled: enabled,
        soundVolume: volume,
      });
      setSoundEnabled(settings.soundEnabled);
      setSoundVolume(settings.soundVolume);
    } catch (reason) {
      setSoundError(String(reason));
    }
  }

  async function refreshSoundCues() {
    setSoundCues(await invoke<SoundCue[]>("list_sound_cues"));
  }

  async function handleSoundImport(sound: SoundCueImport) {
    setSoundBusy(true);
    setSoundError("");
    try {
      await invoke<SoundCue>("import_sound_cue", sound);
      await refreshSoundCues();
    } catch (reason) {
      const message = String(reason);
      setSoundError(message);
      throw new Error(message);
    } finally {
      setSoundBusy(false);
    }
  }

  async function handleSoundDelete(soundCueId: string) {
    setSoundBusy(true);
    setSoundError("");
    try {
      await invoke("delete_sound_cue", { id: soundCueId });
      const [, reminders] = await Promise.all([
        refreshSoundCues(),
        invoke<Reminder[]>("list_reminders"),
      ]);
      setDashboardReminders(reminders);
    } catch (reason) {
      setSoundError(String(reason));
    } finally {
      setSoundBusy(false);
    }
  }

  async function relaunchActivePet() {
    if (!petActive) return;
    await invoke("show_pet", {
      title: null,
      message: "",
      showAfterSeconds: 0,
      visibleForSeconds: 2,
      showBubble: false,
      petSize,
      bubbleStyle,
      reminderAnimation: null,
      soundCueId: null,
      soundVolume: null,
    });
  }

  async function refreshCharacters() {
    const savedCharacters = await invoke<CharacterPack[]>("list_characters");
    setCharacters(savedCharacters);
  }

  async function handleCharacterCreate(name: string) {
    setCharacterBusy(true);
    setCharacterError("");
    try {
      await invoke<CharacterPack>("create_character", { name });
      await refreshCharacters();
    } catch (reason) {
      const message = String(reason);
      setCharacterError(message);
      throw new Error(message);
    } finally {
      setCharacterBusy(false);
    }
  }

  async function handleCharacterAnimationAdd(
    characterId: string,
    animation: string,
    files: CharacterAnimationImportFile[],
  ) {
    setCharacterBusy(true);
    setCharacterError("");
    try {
      await invoke<CharacterPack>("add_character_animation", {
        characterId,
        animation,
        files,
      });
      await refreshCharacters();
      if (characterId === activeCharacterId) {
        await relaunchActivePet();
      }
    } catch (reason) {
      const message = String(reason);
      setCharacterError(message);
      throw new Error(message);
    } finally {
      setCharacterBusy(false);
    }
  }

  async function handleCharacterAnimationDelete(
    characterId: string,
    animationId: string,
  ) {
    setCharacterBusy(true);
    setCharacterError("");
    try {
      const settings = await invoke<PetSettings>(
        "delete_character_animation",
        { characterId, animationId },
      );
      setActiveCharacterId(settings.activeCharacterId);
      await refreshCharacters();
      if (characterId === activeCharacterId) {
        await relaunchActivePet();
      }
    } catch (reason) {
      const message = String(reason);
      setCharacterError(message);
      throw new Error(message);
    } finally {
      setCharacterBusy(false);
    }
  }

  async function handleCharacterSelect(characterId: string) {
    setCharacterBusy(true);
    setCharacterError("");
    try {
      const settings = await invoke<PetSettings>("set_active_character", {
        characterId,
      });
      setActiveCharacterId(settings.activeCharacterId);
      await relaunchActivePet();
    } catch (reason) {
      setCharacterError(String(reason));
    } finally {
      setCharacterBusy(false);
    }
  }

  async function handleCharacterDelete(characterId: string) {
    const character = characters.find((item) => item.id === characterId);
    if (!character || !window.confirm(`Delete ${character.name}?`)) return;

    setCharacterBusy(true);
    setCharacterError("");
    try {
      const settings = await invoke<PetSettings>("delete_character", {
        characterId,
      });
      setActiveCharacterId(settings.activeCharacterId);
      await refreshCharacters();
      await relaunchActivePet();
    } catch (reason) {
      setCharacterError(String(reason));
    } finally {
      setCharacterBusy(false);
    }
  }

  return (
    <div className="app-layout">
      {/* Sidebar */}
      <aside className="sidebar">
        <div className="sidebar-logo">
          <div className="logo-icon">🐑</div>
          <div>
            <div className="logo-text">Baalert</div>
            <div className="logo-sub">CALENDAR ALERTS</div>
          </div>
        </div>

        <span className="nav-section-label">Main</span>

        {navItems.map((item) => (
          <div
            key={item.id}
            className={`nav-item ${activePage === item.id ? "active" : ""}`}
            onClick={() => setActivePage(item.id)}
          >
            <span className="nav-icon">{item.icon}</span>
            {item.label}
            {item.badge && !item.soon && (
              <span className="nav-badge">{item.badge}</span>
            )}
            {item.soon && <span className="nav-soon">soon</span>}
          </div>
        ))}

        <span className="nav-section-label" style={{ marginTop: 8 }}>
          System
        </span>
        <div className="nav-item">
          <span className="nav-icon">📊</span>
          Activity
          <span className="nav-soon">soon</span>
        </div>
        <div className="nav-item">
          <span className="nav-icon">🔒</span>
          Privacy
          <span className="nav-soon">soon</span>
        </div>
      </aside>

      {/* Main */}
      <div className="main-content">
        {/* Topbar */}
        <div className="topbar">
          <div>
            <div className="topbar-title">
              {activePage === "dashboard" && "Dashboard"}
              {activePage === "calendar" && "Calendar"}
              {activePage === "reminders" && "Reminders"}
              {activePage === "settings" && "Settings"}
            </div>
            <div className="topbar-subtitle">
              {new Date().toLocaleDateString("en-US", {
                weekday: "long",
                year: "numeric",
                month: "long",
                day: "numeric",
              })}
            </div>
          </div>
          <div className="topbar-right">
            <button
              className="theme-toggle-button"
              type="button"
              aria-label={darkMode ? "Switch to light mode" : "Switch to dark mode"}
              aria-pressed={darkMode}
              title={darkMode ? "Light mode" : "Dark mode"}
              onClick={() => void handleDarkModeToggle()}
            >
              <span aria-hidden="true">{darkMode ? "☀" : "☾"}</span>
            </button>
            <div className="status-pill">
              <span className={`dot ${petActive ? "" : "inactive"}`} />
              {petActive ? "Pet Active" : "Pet Off"}
            </div>
          </div>
        </div>

        {/* Page Content */}
        <div className="page-content">
          {activePage === "dashboard" && (
            <>
              {/* Stats Row */}
              <div className="stats-row">
                <div className="stat-card">
                  <div className="stat-label">Today's Events</div>
                  <div className="stat-value accent">4</div>
                  <div className="stat-sub">Next in 2 hours</div>
                </div>
                <div className="stat-card">
                  <div className="stat-label">Reminders Set</div>
                  <div className="stat-value">{dashboardReminders.length}</div>
                  <div className="stat-sub">
                    {activeReminderCount} currently active
                  </div>
                </div>
                <div className="stat-card">
                  <div className="stat-label">Alerts Sent</div>
                  <div className="stat-value success">12</div>
                  <div className="stat-sub">This week</div>
                </div>
                <div className="stat-card">
                  <div className="stat-label">Missed Events</div>
                  <div className="stat-value warning">0</div>
                  <div className="stat-sub">All caught up 🎉</div>
                </div>
              </div>

              {/* Desktop Pet Control */}
              <div className="banner-control-card">
                <div className="banner-control-header">
                  <div className="banner-control-info">
                    <h2>Desktop Pet</h2>
                    <p>
                      Keep your character and its reminders visible above your
                      workspace, including fullscreen apps.
                    </p>
                  </div>
                  <div className="toggle-wrapper">
                    <span className={`toggle-label ${petActive ? "on" : ""}`}>
                      {loading ? "..." : petActive ? "ON" : "OFF"}
                    </span>
                    <label className="toggle" id="pet-toggle">
                      <input
                        type="checkbox"
                        checked={petActive}
                        disabled={loading}
                        onChange={(e) => handlePetToggle(e.target.checked)}
                      />
                      <div className="toggle-track" />
                      <div className="toggle-thumb" />
                    </label>
                  </div>
                </div>

                <div className="banner-preview-area">
                  <span className="preview-label">Preview</span>
                  <div className="pet-preview pet-only-preview">
                    <img
                      src={activeCharacter?.previewDataUrl || petIdle}
                      alt="Character preview"
                      style={{
                        width: Math.round(petSize * 0.55),
                        height: Math.round(petSize * 0.55),
                      }}
                    />
                  </div>
                </div>
              </div>

              {/* Bottom grid */}
              <div className="content-grid">
                {/* Upcoming Events */}
                <div className="placeholder-card">
                  <h3>
                    <span>📅</span> Upcoming Events
                    <span className="coming-soon-badge">Coming Soon</span>
                  </h3>
                  <p>
                    Your calendar events will appear here once calendar
                    integration is set up.
                  </p>
                  <div style={{ flex: 1 }}>
                    {placeholderEvents.map((ev, i) => (
                      <div className="event-row" key={i}>
                        <div
                          className="event-color-dot"
                          style={{ background: ev.color }}
                        />
                        <div className="event-info">
                          <div className="event-title">{ev.title}</div>
                          <div className="event-time">{ev.time}</div>
                        </div>
                        <span className="event-time-tag">{ev.tag}</span>
                      </div>
                    ))}
                  </div>
                </div>

                {/* Reminders */}
                <div
                  className="placeholder-card reminder-dashboard-card"
                  role="button"
                  tabIndex={0}
                  onClick={() => setActivePage("reminders")}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      setActivePage("reminders");
                    }
                  }}
                >
                  <h3>
                    <span>🔔</span> Reminders
                  </h3>
                  <p>
                    Schedule repeating desktop-pet messages by minute or hour.
                  </p>
                  <div className="dashboard-reminder-list">
                    {dashboardReminders.length === 0 && (
                      <div className="dashboard-reminder-empty">
                        No reminders scheduled.
                      </div>
                    )}
                    {dashboardReminders.slice(0, 4).map((reminder) => (
                      <div className="dashboard-reminder-row" key={reminder.id}>
                        <span
                          className={`dashboard-reminder-state ${
                            reminder.enabled ? "" : "paused"
                          }`}
                        />
                        <div>
                          <strong>{reminder.title}</strong>
                          <span>
                            Every {reminder.intervalValue} {reminder.intervalUnit}
                            {" · "}
                            {nextRunLabel(reminder)}
                          </span>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                {/* Background service */}
                <div className="placeholder-card">
                  <h3>
                    <span>⚙️</span> Background Service
                    <span className="coming-soon-badge">Coming Soon</span>
                  </h3>
                  <p>
                    Run Baalert in the background even when the app is
                    minimized.
                  </p>
                  <div className="placeholder-body">
                    <span className="ph-icon">🖥️</span>
                    <span className="ph-text">
                      System tray & background polling.
                      <br />
                      Autostart on login support.
                    </span>
                  </div>
                </div>
              </div>
            </>
          )}

          {activePage === "reminders" && (
            <RemindersPage
              onRemindersChange={setDashboardReminders}
              animations={
                activeCharacter?.animations ?? BUILTIN_CHARACTER.animations
              }
              soundCues={soundCues}
              soundVolume={soundVolume}
            />
          )}

          {activePage === "settings" && (
            <SettingsPage
              petSize={petSize}
              bubbleStyle={bubbleStyle}
              soundEnabled={soundEnabled}
              soundVolume={soundVolume}
              soundCues={soundCues}
              soundBusy={soundBusy}
              soundError={soundError}
              characters={characters}
              activeCharacterId={activeCharacterId}
              characterBusy={characterBusy}
              characterError={characterError}
              onPetSizePreview={setPetSize}
              onPetSizeCommit={(size) => void handlePetSizeCommit(size)}
              onBubbleStyleChange={(style) =>
                void handleBubbleStyleChange(style)
              }
              onSoundSettingsChange={handleSoundSettingsChange}
              onSoundImport={handleSoundImport}
              onSoundDelete={handleSoundDelete}
              onCharacterCreate={handleCharacterCreate}
              onCharacterAnimationAdd={handleCharacterAnimationAdd}
              onCharacterAnimationDelete={handleCharacterAnimationDelete}
              onCharacterSelect={handleCharacterSelect}
              onCharacterDelete={handleCharacterDelete}
            />
          )}

          {/* Other pages placeholder */}
          {activePage !== "dashboard" &&
            activePage !== "reminders" &&
            activePage !== "settings" && (
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                justifyContent: "center",
                height: "60vh",
                gap: 12,
                color: "var(--text-muted)",
              }}
            >
              <div style={{ fontSize: 48, opacity: 0.3 }}>
                {activePage === "calendar" && "📅"}
              </div>
              <div
                style={{
                  fontSize: 16,
                  fontWeight: 600,
                  color: "var(--text-secondary)",
                }}
              >
                {activePage.charAt(0).toUpperCase() + activePage.slice(1)} —
                Coming Soon
              </div>
              <div
                style={{
                  fontSize: 13,
                  textAlign: "center",
                  maxWidth: 300,
                }}
              >
                This section is under construction. Head back to Dashboard to
                manage your desktop pet.
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function App() {
  const isPetMode =
    new URLSearchParams(window.location.search).get("mode") === "pet";

  return isPetMode ? <PetOverlay /> : <Dashboard />;
}

export default App;
