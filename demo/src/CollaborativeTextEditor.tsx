import { useLayoutEffect, useRef } from "react";
import {
  anchorSelection,
  resolveSelection,
  type TextCrdtState,
  type TextSelectionOffsets,
} from "@owebeeone/glial-runtime";

export interface CollaborativeTextEditorProps {
  state: TextCrdtState;
  onChange(value: string): void;
  placeholder?: string;
}

function selectionOffsets(root: HTMLElement): TextSelectionOffsets | null {
  const selection = root.ownerDocument.getSelection();
  if (!selection?.anchorNode || !selection.focusNode) return null;
  if (!root.contains(selection.anchorNode) || !root.contains(selection.focusNode)) return null;

  const offset = (node: Node, nodeOffset: number): number => {
    const range = root.ownerDocument.createRange();
    range.selectNodeContents(root);
    range.setEnd(node, nodeOffset);
    return range.toString().length;
  };
  return {
    anchor: offset(selection.anchorNode, selection.anchorOffset),
    focus: offset(selection.focusNode, selection.focusOffset),
  };
}

function domPoint(root: HTMLElement, rawOffset: number): { node: Node; offset: number } {
  const offset = Math.max(0, Math.min(root.textContent?.length ?? 0, rawOffset));
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let remaining = offset;
  let last: Text | null = null;
  for (let node = walker.nextNode() as Text | null; node; node = walker.nextNode() as Text | null) {
    last = node;
    if (remaining <= node.data.length) return { node, offset: remaining };
    remaining -= node.data.length;
  }
  if (last) return { node: last, offset: last.data.length };
  return { node: root, offset: 0 };
}

function restoreSelection(root: HTMLElement, offsets: TextSelectionOffsets): void {
  const selection = root.ownerDocument.getSelection();
  if (!selection) return;
  const anchor = domPoint(root, offsets.anchor);
  const focus = domPoint(root, offsets.focus);
  selection.setBaseAndExtent(anchor.node, anchor.offset, focus.node, focus.offset);
}

function editorText(root: HTMLElement): string {
  // `plaintext-only` keeps this as text rather than browser-authored markup.
  // innerText preserves line breaks produced by Enter in contenteditable.
  return root.innerText.replace(/\r\n/g, "\n");
}

/**
 * An uncontrolled plaintext DOM editor backed by identity operations.
 *
 * React never assigns a `value`, so remote updates cannot trigger the native
 * input behavior that moves selection to the end. Immediately before a CRDT
 * projection changes the DOM, the selection is converted from offsets to
 * element-id anchors; after the delta it is resolved back into the new DOM.
 */
export function CollaborativeTextEditor({ state, onChange, placeholder = "shared CRDT notes…" }: CollaborativeTextEditorProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const previous = useRef(state);
  const pendingLocalSelection = useRef<TextSelectionOffsets | null>(null);
  const composing = useRef(false);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const local = pendingLocalSelection.current;
    const remoteOffsets = root.ownerDocument.activeElement === root ? selectionOffsets(root) : null;
    const anchored = local
      ? anchorSelection(state, local)
      : remoteOffsets
        ? anchorSelection(previous.current, remoteOffsets)
        : null;

    if (root.textContent !== state.text) root.textContent = state.text;
    if (anchored) restoreSelection(root, resolveSelection(state, anchored));
    previous.current = state;
    pendingLocalSelection.current = null;
  }, [state]);

  const commitDom = () => {
    const root = rootRef.current;
    if (!root) return;
    pendingLocalSelection.current = selectionOffsets(root);
    onChange(editorText(root));
  };

  return (
    <div
      ref={rootRef}
      className="collaborative-editor"
      role="textbox"
      aria-label="Collaborative CRDT notes"
      aria-multiline="true"
      contentEditable="plaintext-only"
      suppressContentEditableWarning
      spellCheck
      data-placeholder={placeholder}
      onCompositionStart={() => { composing.current = true; }}
      onCompositionEnd={() => {
        composing.current = false;
        commitDom();
      }}
      onInput={() => {
        if (!composing.current) commitDom();
      }}
    />
  );
}
