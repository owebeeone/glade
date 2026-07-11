// The Chat tab's wiring (GLP-0006 P1.S1) — grip-style, no React state hook for
// shared state. The group surfaces are declared via the glade-chat manifest
// (chatManifest built from CHAT_GROUPS config); each group is a keyed commons
// log (chat.msgs, group id = key) mounted as a glial tap producing ChatLine[].
//
// Stage-1 chat is CLIENT appends + node fold/replicate: posting a line stamps a
// ChatLine (attribution = the participant, via glade-chat's postChat) and
// appends it through the group's glial log controller — the supplier is not in
// the message path. #general and #dev are distinct keyed surfaces, so a #general
// subscriber never receives #dev (isolation by keying).

import { type Grip, createAtomValueTap } from "@owebeeone/grip-react";
import { glialTap } from "@owebeeone/glial-runtime/grip";
import { SessionDestination, type SessionLike } from "@owebeeone/glial-runtime";
import {
  chatManifest,
  groupKey,
  postChat,
  type ChatGroup,
  type ChatLine,
} from "@owebeeone/glade-chat";
import { defineGrip, grok, main } from "./runtime";
import { CHATLINE_CODEC, bus, glial, session, user } from "./glial";
import type { Surface } from "./manifest";

/** The pre-declared chat groups — stage-1 config (the supplier serves this list
 *  as chat.groups; the demo selector reads the config directly). Dynamic
 *  creation is a create-a-share ceremony that rides F2 + P2. */
export const CHAT_GROUPS: ChatGroup[] = [
  { id: "general", label: "#general" },
  { id: "dev", label: "#dev" },
];

/** The typed chat surfaces (share "chat"; chat.msgs keyed per group). */
const chatM = chatManifest(CHAT_GROUPS, { share: "chat" });

/** The selected group — grip-style shared state (no React state hook), like
 *  CURRENT_TAB. The *_TAP handle lets the group selector switch it. */
export const CHAT_GROUP = defineGrip<string>("ChatGroup", CHAT_GROUPS[0]!.id);
export const CHAT_GROUP_TAP = defineGrip<any>("ChatGroup.tap", undefined);

interface GroupGrips {
  readonly list: Grip<ChatLine[]>;
  readonly tap: Grip<any>;
}
const groupGrips = new Map<string, GroupGrips>(
  CHAT_GROUPS.map((g) => [
    g.id,
    { list: defineGrip<ChatLine[]>(`Chat.${g.id}`, []), tap: defineGrip<any>(`Chat.${g.id}.tap`, undefined) },
  ]),
);

/** The line-list grip for a group (the panel reads the selected group's). */
export function groupListGrip(id: string): Grip<ChatLine[]> {
  const gg = groupGrips.get(id);
  if (!gg) throw new Error(`chat: unknown group '${id}'`);
  return gg.list;
}

/** The wire destination for a group's keyed commons log (share "chat"). The
 *  route is fully carried by the typed surface — no manifestScope needed. */
function chatDest(surface: Surface): () => SessionDestination {
  const route = {
    share: surface.share,
    gladeId: surface.glade_id.id,
    shape: surface.shape,
    key: surface.key ?? new Uint8Array(), // a group always carries its key
  };
  return () => new SessionDestination(session as unknown as SessionLike, bus, route);
}

/** Register the Chat tab's taps: the selected-group atom + one glial log mount
 *  per group (both subscribed; the panel shows the selected one). */
export function registerChatTaps(): void {
  grok.registerTap(
    createAtomValueTap(CHAT_GROUP, { initial: CHAT_GROUPS[0]!.id, handleGrip: CHAT_GROUP_TAP }) as never,
  );
  for (const g of CHAT_GROUPS) {
    const surface = chatM.msg(g.id);
    const gg = groupGrips.get(g.id)!;
    grok.registerTap(
      glialTap<ChatLine[]>({
        binder: glial,
        decl: surface,
        grip: gg.list,
        // distinct fill per group → distinct instance (same glade id, group key).
        fill: { domain: g.id, zone: "commons" },
        codec: CHATLINE_CODEC,
        handleGrip: gg.tap,
        gladeFor: chatDest(surface),
      }) as never,
    );
  }
}

/** The chat surfaces the demo subscribes on connect (node interest + history
 *  replay for late joiners) — each group's keyed commons log. */
export function chatSubscriptions(): Array<{ share: string; gladeId: string; key: Uint8Array }> {
  return CHAT_GROUPS.map((g) => {
    const s = chatM.msg(g.id);
    return { share: s.share, gladeId: s.glade_id.id, key: groupKey(g.id) };
  });
}

const ctlCache = new Map<string, { append(entry: unknown): void }>();

/** Post a line to a group, attributed to the participant. Stage-1 CLIENT append
 *  through the group's glial log controller — stamps ChatLine{user, principal,
 *  ts, text} (principal = the demo user, the P0.S7 stub replacement). */
export function postToGroup(groupId: string, text: string): void {
  let ctl = ctlCache.get(groupId);
  if (!ctl) {
    const gg = groupGrips.get(groupId);
    if (!gg) throw new Error(`chat: unknown group '${groupId}'`);
    const drip = grok.query(gg.tap, main) as { get(): { append(entry: unknown): void } | undefined };
    grok.flush();
    const resolved = drip.get();
    if (!resolved) throw new Error(`chat controller not ready for '${groupId}' (${gg.tap.key} unresolved)`);
    ctl = resolved;
    ctlCache.set(groupId, ctl);
  }
  // glade-chat's client helper stamps + appends; user = principal in stage 1.
  postChat(ctl as { append(line: ChatLine): void }, text, user);
}
