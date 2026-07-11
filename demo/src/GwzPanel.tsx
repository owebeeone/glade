// The Gwz tab (GLP-0006 P1.S3) — a thin projection over grips, no React state
// hook for shared state. The verb is a grip atom (GWZ_VERB, picker-limited to
// the allow-list); the last answer is GWZ_RESULT; the streamed run is GWZ_STREAM
// (a live glial mount on gwz.output). The args box is uncontrolled (read on run).
import { useGrip } from "@owebeeone/grip-react";
import {
  GWZ_RESULT,
  GWZ_STREAM,
  GWZ_VERB,
  GWZ_VERB_TAP,
  GWZ_VERBS,
  runGwz,
  streamGwz,
} from "./gwz";
import { user } from "./glade";

/** Split the args box into an argv (whitespace-separated, empties dropped). */
function readArgs(form: HTMLFormElement): string[] {
  const input = form.elements.namedItem("args") as HTMLInputElement | null;
  return (input?.value ?? "").trim().split(/\s+/).filter(Boolean);
}

const box: React.CSSProperties = { whiteSpace: "pre-wrap", fontFamily: "ui-monospace, monospace" };

export function GwzPanel() {
  const verb = useGrip(GWZ_VERB) ?? GWZ_VERBS[0];
  const verbTap = useGrip(GWZ_VERB_TAP);
  const result = useGrip(GWZ_RESULT);
  const stream = useGrip(GWZ_STREAM) ?? [];

  return (
    <div className="panel">
      <header>
        <h1>Gwz</h1>
        <div className="meta">
          <b>ws-razel · gwz.ops</b> · you are <b>{user}</b>
        </div>
      </header>

      <section>
        <h2>Verb · read-only allow-list</h2>
        <div className="files">
          {GWZ_VERBS.map((v) => (
            <button
              key={v}
              className={v === verb ? "file active" : "file"}
              onClick={() => verbTap?.set(v)}
              disabled={v === verb}
            >
              {v}
            </button>
          ))}
        </div>
        <div className="current zone-commons">
          the supplier runs allow-listed read verbs against the app-owned workspace; a mutating verb is
          refused as data.
        </div>
      </section>

      <section>
        <h2>Run · exchange</h2>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void runGwz(verb, readArgs(e.currentTarget));
          }}
        >
          <input name="args" placeholder={`args for ${verb} (optional)…`} autoComplete="off" />
          <button type="submit">run</button>
          <button
            type="button"
            onClick={(e) => void streamGwz(verb, readArgs(e.currentTarget.form!))}
          >
            stream
          </button>
          <button
            type="button"
            title="send a disallowed (mutating) verb — proves failure-as-data"
            onClick={() => void runGwz("commit", ["-m", "demo"])}
          >
            deny demo
          </button>
        </form>

        {result && (
          <div className={`current ${result.ok ? "zone-commons" : "zone-private"}`}>
            {result.error ? (
              <>
                error: <b>{result.error}</b>
              </>
            ) : (
              <>
                ok=<b>{String(result.ok)}</b>
                {result.exit != null && (
                  <>
                    {" · "}exit=<b>{result.exit}</b>
                  </>
                )}
                {result.run_id && (
                  <>
                    {" · "}run <b>{result.run_id}</b>
                  </>
                )}
                {result.attributed_to && (
                  <>
                    {" · by "}
                    <b>{result.attributed_to}</b>
                  </>
                )}
              </>
            )}
          </div>
        )}
        {result?.stdout && (
          <div className="activity" style={box}>
            {result.stdout}
          </div>
        )}
        {result?.stderr && (
          <div className="activity zone-private" style={box}>
            {result.stderr}
          </div>
        )}
      </section>

      <section>
        <h2>Stream · gwz.output log</h2>
        <div className="activity">
          {stream.length === 0 && (
            <div className="empty">no streamed run yet — press “stream” to watch gwz.output converge live</div>
          )}
          {stream.map((r, i) =>
            r.stream === "end" ? (
              <div key={i} className="entry">
                <b>— done</b> · exit {r.exit}
              </div>
            ) : (
              <div key={i} className="entry">
                <span className="ts">{r.seq}</span> <b>{r.stream}</b> {r.line}
              </div>
            ),
          )}
        </div>
        <div className="current zone-commons">
          keyed by run id — the supplier appends each line as a log op that folds + replicates to this mount.
        </div>
      </section>
    </div>
  );
}
