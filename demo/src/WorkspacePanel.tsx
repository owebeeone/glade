import { useEffect, useState } from "react";
import { useGrip } from "@owebeeone/grip-react";
import { SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY } from "./grips";
import { origin, postActivity, onStatus, type GladeStatus } from "./glade";

const FILES = ["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"];

export function WorkspacePanel() {
  const selection = useGrip(SELECTION);
  const selTap = useGrip(SELECTION_TAP);
  const notes = useGrip(NOTES);
  const notesTap = useGrip(NOTES_TAP);
  const activity = useGrip(ACTIVITY) ?? [];
  const [msg, setMsg] = useState("");
  const [status, setStatus] = useState<GladeStatus>("connecting");
  useEffect(() => onStatus(setStatus), []);

  return (
    <div className="panel">
      <header>
        <h1>Gryth Workspace</h1>
        <div className="meta">
          <span className={`dot ${status}`} /> {status} · you are <b>{origin}</b>
        </div>
      </header>

      <section>
        <h2>Selection</h2>
        <div className="files">
          {FILES.map((f) => (
            <button
              key={f}
              className={f === selection ? "file active" : "file"}
              onClick={() => {
                selTap?.set(f);
                postActivity(`${origin} selected ${f}`);
              }}
            >
              {f}
            </button>
          ))}
        </div>
        <div className="current">current: <b>{selection || "(none)"}</b></div>
      </section>

      <section>
        <h2>Notes</h2>
        <textarea
          value={notes ?? ""}
          rows={4}
          placeholder="shared notes…"
          onChange={(e) => notesTap?.set(e.target.value)}
        />
      </section>

      <section>
        <h2>Activity</h2>
        <div className="activity">
          {activity.length === 0 && <div className="empty">no activity yet</div>}
          {activity.map((a, i) => (
            <div key={i} className="entry">{a}</div>
          ))}
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (msg.trim()) {
              postActivity(`${origin}: ${msg.trim()}`);
              setMsg("");
            }
          }}
        >
          <input value={msg} placeholder="post to activity…" onChange={(e) => setMsg(e.target.value)} />
          <button type="submit">post</button>
        </form>
      </section>
    </div>
  );
}
