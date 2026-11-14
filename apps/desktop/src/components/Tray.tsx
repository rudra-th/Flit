import { bytes } from "../lib/format";
import type { TrayEntry } from "../lib/types";
import { CloseIcon, DropIcon, FileIcon, FolderIcon, PlusIcon } from "./Icons";

/** What is about to go. A full-width panel under the devices, because the two
 * halves of the decision — where and what — belong in one column and one size.
 *
 * Empty, it is a drop target as wide as the window; full, it is a list, a
 * total and the one accent button on the screen. */
export function Tray({
  entries,
  target,
  canSend,
  onAdd,
  onRemove,
  onClear,
  onSend,
}: {
  entries: TrayEntry[];
  target: string | null;
  canSend: boolean;
  onAdd: () => void;
  onRemove: (entry: TrayEntry) => void;
  onClear: () => void;
  onSend: () => void;
}) {
  const total = entries.reduce((sum, entry) => sum + entry.bytes, 0);
  const files = entries.reduce((sum, entry) => sum + entry.files, 0);
  const label =
    entries.length === 0
      ? "nothing yet"
      : entries.length === 1
        ? "1 item"
        : `${entries.length} items`;
  // Pasted text is one item with a name, not a file with a tree; the footer
  // counts it honestly rather than calling a link "1 file".
  const hasText = entries.some((entry) => entry.text !== undefined);

  return (
    <section className="panel tray" aria-label="Files to send">
      <div className="panel-head">
        <span className="panel-title">To send</span>
        <span className="panel-meta">{label}</span>
        {entries.length > 0 ? (
          <button
            type="button"
            className="btn btn-quiet tray-clear"
            onClick={onClear}
          >
            Clear
          </button>
        ) : null}
      </div>

      <div className="tray-body">
        {entries.length === 0 ? (
          <button type="button" className="tray-empty" onClick={onAdd}>
            <span className="drop-mark" aria-hidden>
              <DropIcon size={18} />
            </span>
            <span>
              Drop files or folders anywhere in this window, or{" "}
              <strong>choose them</strong>.
            </span>
            <span className="spacer" />
            <span className="btn btn-quiet">
              <PlusIcon size={13} />
              Add files
            </span>
          </button>
        ) : (
          <div className="tray-list">
            {entries.map((entry) => (
              <div className="tray-item" key={keyOf(entry)}>
                <span className="tray-icon" aria-hidden>
                  {entry.isDir ? <FolderIcon size={13} /> : <FileIcon size={13} />}
                </span>
                <span className="name" title={entry.text ?? entry.path}>
                  {entry.name}
                  {entry.text !== undefined ? (
                    <span className="peek"> · {peek(entry.text)}</span>
                  ) : null}
                </span>
                <span className="size">
                  {entry.isDir ? `${entry.files} files · ` : ""}
                  {bytes(entry.bytes)}
                </span>
                <button
                  type="button"
                  className="remove"
                  aria-label={
                    entry.text === undefined
                      ? `Remove ${entry.name}`
                      : "Remove pasted text"
                  }
                  onClick={() => onRemove(entry)}
                >
                  <CloseIcon size={12} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>

      {entries.length > 0 ? (
        <div className="tray-foot">
          <div className="tray-total">
            <span>
              {files} {hasText ? (files === 1 ? "item" : "items") : files === 1 ? "file" : "files"}
            </span>
            <span className="sum">{bytes(total)}</span>
          </div>
          <button
            type="button"
            className="btn btn-accent"
            disabled={!canSend}
            onClick={onSend}
            title={canSend ? undefined : "Choose a device first"}
          >
            <span className="label">
              {target === null ? "Choose a device" : `Send to ${target}`}
            </span>
          </button>
        </div>
      ) : null}
    </section>
  );
}

/** A row's identity: a path, or the text itself (already deduplicated, so two
 * pasted items can never share it). */
function keyOf(entry: TrayEntry): string {
  return entry.text ?? entry.path;
}

/** The first line of pasted text, short enough to sit beside a file name. */
function peek(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > 52 ? `${flat.slice(0, 52)}…` : flat;
}