import { useEffect, useState } from "react";
import { useGrip } from "@owebeeone/grip-react";
import {
  SELECTION, SELECTION_TAP, NOTES, NOTES_TAP, ACTIVITY, STATUS, STATUS_TAP,
  FILE_WINDOW, FILE_WINDOW_TAP,
  COLLABORATIVE_NOTES, COLLABORATIVE_NOTES_TAP,
} from "./grips";
import { doc, user, postActivity, onStatus, type GladeStatus } from "./glade";
import { emptyTextCrdtState, fromUtf8 } from "@owebeeone/glial-runtime";
import { EMPTY_FILE_WINDOW } from "./files";
import { CollaborativeTextEditor } from "./CollaborativeTextEditor";

const PATHS = ["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"];

export function WorkspacePanel() {
  const selection = useGrip(SELECTION);
  const selTap = useGrip(SELECTION_TAP);
  const notes = useGrip(NOTES);
  const notesTap = useGrip(NOTES_TAP);
  const activity = useGrip(ACTIVITY) ?? [];
  const status_ = useGrip(STATUS);
  const statusTap = useGrip(STATUS_TAP);
  const fileWindow = useGrip(FILE_WINDOW) ?? EMPTY_FILE_WINDOW;
  const fileTap = useGrip(FILE_WINDOW_TAP);
  const collaborativeNotes = useGrip(COLLABORATIVE_NOTES) ?? emptyTextCrdtState();
  const collaborativeNotesTap = useGrip(COLLABORATIVE_NOTES_TAP);
  const [msg, setMsg] = useState("");
  const [fileError, setFileError] = useState("");
  const [conn, setConn] = useState<GladeStatus>("connecting");
  useEffect(() => onStatus(setConn), []);

  return (
    <div className="panel">
      <header>
        <h1>Gryth Workspace</h1>
        <div className="meta">
          <span className={`dot ${conn}`} /> {conn} · <b>doc:{doc}</b> · you are <b>{user}</b>
        </div>
      </header>

      <section>
        <h2>Status · account domain</h2>
        <input
          value={status_ ?? ""}
          placeholder="your status (follows you across documents)…"
          onChange={(e) => statusTap?.set(e.target.value)}
        />
        <div className="current zone-account">account:{user} · commons — same on every doc you open</div>
      </section>

      <section>
        <h2>Selection · private zone</h2>
        <div className="files">
          {PATHS.map((f) => (
            <button
              key={f}
              className={f === selection ? "file active" : "file"}
              onClick={() => {
                selTap?.set(f);
                postActivity(`selected ${f}`);
              }}
            >
              {f}
            </button>
          ))}
        </div>
        <div className="current zone-private">
          self:{user} · private — only you see this, even when the doc is shared: <b>{selection || "(none)"}</b>
        </div>
      </section>

      <section>
        <h2>File window · canonical SWMR</h2>
        <textarea
          value={fromUtf8(fileWindow.bytes)}
          rows={7}
          placeholder="install the first ws.files snapshot…"
          onChange={(e) => {
            try {
              const needsSnapshot = fileWindow.revision.endsWith(":empty") || fileWindow.revision.endsWith(":reset");
              if (needsSnapshot) fileTap?.snapshot(e.target.value);
              else fileTap?.delta(e.target.value);
              setFileError("");
            } catch (error) {
              setFileError(error instanceof Error ? error.message : String(error));
            }
          }}
        />
        <div className="files">
          <button onClick={() => fileTap?.snapshot("// demo.txt\ncanonical SWMR generation\n")}>
            install snapshot
          </button>
          <button onClick={() => fileTap?.reset("workspace generation changed")}>reset generation</button>
        </div>
        <div className="current zone-commons">
          ws.files · demo.txt (path routing pending) · revision <b>{fileWindow.revision}</b> · {fileWindow.length}/{fileWindow.total} bytes
        </div>
        {fileError && <div className="empty">{fileError}</div>}
      </section>

      <section>
        <h2>Collaborative notes · text CRDT</h2>
        <CollaborativeTextEditor
          state={collaborativeNotes}
          onChange={(value) => collaborativeNotesTap?.replaceText(value)}
        />
        <div className="current zone-commons">
          doc:{doc} · commons · {collaborativeNotes.visible.length} live elements · cursor anchored by element identity
        </div>
        {collaborativeNotes.diagnostics.length > 0 && (
          <div className="empty">CRDT diagnostics: {collaborativeNotes.diagnostics.join(", ")}</div>
        )}
      </section>

      <section>
        <h2>Notes · commons zone</h2>
        <textarea
          value={notes ?? ""}
          rows={4}
          placeholder="shared notes…"
          onChange={(e) => notesTap?.set(e.target.value)}
        />
        <div className="current zone-commons">doc:{doc} · commons — whole-value LWW comparison</div>
      </section>

      <section>
        <h2>Activity · commons zone</h2>
        <div className="activity">
          {activity.length === 0 && <div className="empty">no activity yet</div>}
          {activity.map((a, i) => (
            <div key={i} className="entry">
              <span className="ts">{new Date(a.ts).toLocaleTimeString()}</span>{" "}
              <b>{a.user}</b> {a.text}
            </div>
          ))}
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (msg.trim()) {
              postActivity(msg.trim());
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
