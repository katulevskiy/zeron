import { useCallback, useEffect, useMemo, useRef } from "react";
import { Editor } from "@pierre/diffs/edit";
import { EditProvider, File, type FileContents, type LineAnnotation } from "@pierre/diffs/react";
import type { ReviewComment } from "../../lib/review-comments";
import { useResolvedAppearance } from "../../state/appearance";
import { EditorCommentCard } from "../review-comments/editor-comment-card";
import { EditorCommentDraft } from "../review-comments/editor-comment-draft";

/** The document and save lifecycle belong to FileDocument, not the rendering library. */
export interface CodeViewProps {
  readonly text: string;
  readonly path: string;
  readonly editable: boolean;
  readonly onChange: (text: string) => void;
  readonly codeFontSize: number;
  readonly wordWrap: boolean;
  readonly autoFocus?: boolean;
  readonly review?: CodeReviewWiring | null;
}

export interface CodeReviewWiring {
  readonly comments: readonly ReviewComment[];
  readonly activeId: string | null;
  readonly draft: { readonly path: string; readonly line: number; readonly body: string; readonly editingId: string | null } | null;
  readonly onOpenDraft: (line: number) => void;
  readonly onToggleActive: (id: string) => void;
  readonly onCardEdit: (id: string) => void;
  readonly onCardRemove: (id: string) => void;
  readonly onDraftBody: (body: string) => void;
  readonly onDraftCancel: () => void;
  readonly onDraftCommit: () => void;
}

/**
 * Pierre owns selection, input, highlighting, wrapping, scrolling and undo.
 * FileDocument remains the source of truth for dirty state and remote saves.
 * Do not feed its per-keystroke snapshot back into Pierre's active edit session:
 * the library explicitly treats the `file` prop as an external document update.
 */
export function CodeView({ text, path, editable, onChange, codeFontSize, wordWrap, autoFocus, review }: CodeViewProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const appearance = useResolvedAppearance();
  const editorRef = useRef<Editor<"file"> | null>(null);
  const lastEditRef = useRef<string | null>(null);
  const fileRef = useRef<FileContents>({ name: path, contents: text });
  if (fileRef.current.name !== path || (!editable && fileRef.current.contents !== text)
    || (editable && text !== lastEditRef.current && text !== fileRef.current.contents)) {
    // A new path, reload, or remote document update, not an echo of onEditChange.
    fileRef.current = { name: path, contents: text };
    lastEditRef.current = null;
  }
  const file = fileRef.current;
  const createEditor = useCallback(<EType extends "file" | "file-diff">(type: EType, options: ConstructorParameters<typeof Editor<EType>>[1], editStateKey?: string) => {
    const editor = new Editor(type, options, editStateKey);
    if (type === "file") editorRef.current = editor as Editor<"file">;
    return editor;
  }, []);
  useEffect(() => {
    if (autoFocus && editable) editorRef.current?.focus();
  }, [autoFocus, editable]);

  const options = useMemo(() => ({
    disableFileHeader: true,
    overflow: wordWrap ? "wrap" as const : "scroll" as const,
    themeType: appearance,
    onLineNumberClick: review ? ({ lineNumber }: { lineNumber: number }) => {
      const existing = review.comments.find((comment) => comment.line === lineNumber);
      if (existing) review.onToggleActive(existing.id);
      else review.onOpenDraft(lineNumber);
    } : undefined,
  }), [wordWrap, review, appearance]);
  const annotations: LineAnnotation<ReviewComment>[] = review?.comments.map((comment) => ({
    lineNumber: comment.line,
    metadata: comment,
  })) ?? [];
  const active = review?.comments.find((comment) => comment.id === review.activeId);
  const draft = review?.draft;

  return (
    <div ref={hostRef} className="files-pierre-code" style={{ fontSize: `${codeFontSize}px` }}>
      <EditProvider createEditor={createEditor}>
        <File<ReviewComment>
          file={file}
          edit={editable}
          options={options}
          editorOptions={{ ownsVerticalViewport: true }}
          lineAnnotations={annotations}
          renderAnnotation={(annotation) => (
            <button type="button" className="files-pierre-comment-link" onClick={() => review?.onToggleActive(annotation.metadata.id)}>
              Comment on line {annotation.lineNumber}
            </button>
          )}
          onEditChange={(event) => {
            lastEditRef.current = event.file.contents;
            onChange(event.file.contents);
          }}
          onEditComplete={() => "reject"}
        />
      </EditProvider>
      {review !== null && review !== undefined && active !== undefined && draft === null ? (
        <EditorCommentCard comment={active} left={48} top={8} width={Math.min(hostRef.current?.clientWidth ?? 360, 360)}
          onEdit={review.onCardEdit} onRemove={review.onCardRemove} />
      ) : null}
      {review !== null && review !== undefined && draft !== null && draft !== undefined ? (
        <EditorCommentDraft left={48} top={8} width={Math.min(hostRef.current?.clientWidth ?? 360, 360)}
          body={draft.body} editing={draft.editingId !== null}
          placeholder="Add a comment…" onBody={review.onDraftBody}
          onCancel={review.onDraftCancel} onCommit={review.onDraftCommit} />
      ) : null}
    </div>
  );
}
