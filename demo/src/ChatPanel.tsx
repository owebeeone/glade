// The Chat tab (GLP-0006 P1.S1) — a thin projection over grips, no React state
// hook. The selected group is a grip atom (CHAT_GROUP); the message list is the
// selected group's glial log mount; the compose box is an uncontrolled input
// (posting is a client append — chat.ts postToGroup stamps the ChatLine).
import { useGrip } from "@owebeeone/grip-react";
import { CHAT_GROUP, CHAT_GROUP_TAP, CHAT_GROUPS, groupListGrip, postToGroup } from "./chat";
import { user } from "./glade";

const labelOf = (id: string) => CHAT_GROUPS.find((g) => g.id === id)?.label ?? id;

export function ChatPanel() {
  const group = useGrip(CHAT_GROUP) ?? CHAT_GROUPS[0]!.id;
  const groupTap = useGrip(CHAT_GROUP_TAP);
  const lines = useGrip(groupListGrip(group)) ?? [];

  return (
    <div className="panel">
      <header>
        <h1>Chat</h1>
        <div className="meta">
          <b>share:chat</b> · you are <b>{user}</b>
        </div>
      </header>

      <section>
        <h2>Groups · keyed commons</h2>
        <div className="files">
          {CHAT_GROUPS.map((g) => (
            <button
              key={g.id}
              className={g.id === group ? "file active" : "file"}
              onClick={() => groupTap?.set(g.id)}
              disabled={g.id === group}
            >
              {g.label}
            </button>
          ))}
        </div>
        <div className="current zone-commons">
          chat.msgs · key <b>{group}</b> — everyone in {labelOf(group)}; a different group is a
          different key on the same surface (isolated by keying).
        </div>
      </section>

      <section>
        <h2>{labelOf(group)} · commons log</h2>
        <div className="activity">
          {lines.length === 0 && <div className="empty">no messages in {labelOf(group)} yet</div>}
          {lines.map((l, i) => (
            <div key={i} className="entry">
              <span className="ts">{new Date(l.ts).toLocaleTimeString()}</span>{" "}
              <b>{l.principal ?? l.user}</b> {l.text}
            </div>
          ))}
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            const input = e.currentTarget.elements.namedItem("msg") as HTMLInputElement;
            const text = input.value.trim();
            if (text) {
              postToGroup(group, text);
              input.value = "";
            }
          }}
        >
          <input name="msg" placeholder={`message ${labelOf(group)}…`} autoComplete="off" />
          <button type="submit">send</button>
        </form>
      </section>
    </div>
  );
}
