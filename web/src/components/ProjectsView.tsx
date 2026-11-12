import { useId, useState, type FormEvent } from "react";
import type { ApiClient } from "../api/client";
import type { Project } from "../api/types";
import { memoriesCount } from "../domain/categories";
import { useProjects, type MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { dangerButton, field, hint, label, primaryButton, secondaryButton } from "./styles";

const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;

/**
 * The user's projects (ai-specs/memory-structure-plan.md §6): what each is
 * called, what else finds it, how many memories it holds. Rename one (its
 * memories follow and the old name keeps finding it), merge two that are
 * one, or open a project's memories in the list.
 */
export function ProjectsView({
  client,
  user,
  canWrite,
  actions,
  onShowMemories,
}: {
  client: ApiClient;
  user: string;
  canWrite: boolean;
  actions: MemoryActions;
  onShowMemories: (project: string) => void;
}) {
  const projects = useProjects(client, user);

  if (projects.isPending) return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading projects…</p>;
  if (projects.isError) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        The projects could not be loaded: {projects.error.message}
      </p>
    );
  }
  if (projects.data.length === 0) {
    return (
      <p className="rounded-lg border border-dashed border-slate-300 p-8 text-center text-sm text-slate-600 dark:border-slate-700 dark:text-slate-400">
        No projects yet. A project is created when a memory is saved for one — by an agent working in a repository, or
        here, by naming it in Add memory.
      </p>
    );
  }

  return (
    <section aria-label="Projects" className="space-y-3">
      <p className="text-sm text-slate-600 dark:text-slate-400">
        A project&apos;s memories store the project, not its name, so renaming it moves nothing and its old name keeps
        finding it. Merge two projects that turn out to be one.
      </p>
      <ul className="space-y-3">
        {projects.data.map((project) => (
          <ProjectRow
            key={project.id}
            project={project}
            others={projects.data.filter((p) => p.id !== project.id)}
            canWrite={canWrite}
            actions={actions}
            onShowMemories={onShowMemories}
          />
        ))}
      </ul>
    </section>
  );
}

function ProjectRow({
  project,
  others,
  canWrite,
  actions,
  onShowMemories,
}: {
  project: Project;
  others: readonly Project[];
  canWrite: boolean;
  actions: MemoryActions;
  onShowMemories: (project: string) => void;
}) {
  const id = useId();
  const { show } = useToasts();
  const [mode, setMode] = useState<"view" | "rename" | "merge">("view");
  const [name, setName] = useState(project.name);
  const [into, setInto] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const close = () => {
    setMode("view");
    setError(null);
    setName(project.name);
    setInto("");
  };

  const rename = async (event: FormEvent) => {
    event.preventDefault();
    const next = name.trim().toLowerCase();
    if (!NAME.test(next)) {
      return setError("Use lowercase letters, digits, “.”, “_” or “-”, like “quill-app”.");
    }
    if (next === project.name) return close();
    setBusy(true);
    const outcome = await actions.renameProject(project.name, next);
    setBusy(false);
    if (outcome.kind === "failed") return setError(outcome.message);
    show({ tone: "success", message: outcome.value.message });
    close();
  };

  const merge = async (event: FormEvent) => {
    event.preventDefault();
    if (into === "") return setError("Choose the project to merge it into.");
    setBusy(true);
    const outcome = await actions.mergeProject(project.name, into);
    setBusy(false);
    if (outcome.kind === "failed") return setError(outcome.message);
    show({ tone: "success", message: outcome.value.message });
  };

  return (
    <li className="rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-800 dark:bg-slate-900">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 className="text-base font-semibold text-slate-900 dark:text-slate-100">{project.name}</h3>
        <span className="text-sm text-slate-600 dark:text-slate-400">{memoriesCount(project.memories)}</span>
      </div>
      {project.aliases.length > 0 && (
        <p className="mt-1 text-xs text-slate-600 dark:text-slate-400">Also found by {project.aliases.join(", ")}</p>
      )}

      {mode === "view" && (
        <div className="mt-3 flex flex-wrap gap-2">
          <button type="button" onClick={() => onShowMemories(project.name)} className={secondaryButton}>
            Show its memories
          </button>
          {canWrite && (
            <>
              <button type="button" onClick={() => setMode("rename")} className={secondaryButton}>
                Rename…
              </button>
              {others.length > 0 && (
                <button type="button" onClick={() => setMode("merge")} className={secondaryButton}>
                  Merge into…
                </button>
              )}
            </>
          )}
        </div>
      )}

      {mode === "rename" && (
        <form onSubmit={(event) => void rename(event)} className="mt-3 space-y-2" noValidate>
          <label htmlFor={`${id}-name`} className={label}>
            New name for {project.name}
          </label>
          <input
            id={`${id}-name`}
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              setError(null);
            }}
            aria-invalid={error !== null}
            aria-describedby={`${id}-name-hint`}
            className={field}
          />
          <p id={`${id}-name-hint`} className={hint}>
            Its memories follow, and “{project.name}” keeps finding it.
          </p>
          <FormActions busy={busy} submit="Rename" onCancel={close} error={error} />
        </form>
      )}

      {mode === "merge" && (
        <form onSubmit={(event) => void merge(event)} className="mt-3 space-y-2" noValidate>
          <label htmlFor={`${id}-into`} className={label}>
            Merge {project.name} into
          </label>
          <select
            id={`${id}-into`}
            value={into}
            onChange={(event) => {
              setInto(event.target.value);
              setError(null);
            }}
            className={field}
          >
            <option value="">Choose a project…</option>
            {others.map((other) => (
              <option key={other.id} value={other.name}>
                {other.name}
              </option>
            ))}
          </select>
          <p className={hint}>
            Its {memoriesCount(project.memories)} move there, and “{project.name}” and what else finds it find that project
            from then on. This cannot be undone here.
          </p>
          <FormActions busy={busy} submit="Merge" danger onCancel={close} error={error} />
        </form>
      )}
    </li>
  );
}

function FormActions({
  busy,
  submit,
  danger = false,
  onCancel,
  error,
}: {
  busy: boolean;
  submit: string;
  danger?: boolean;
  onCancel: () => void;
  error: string | null;
}) {
  return (
    <>
      {error !== null && (
        <p role="alert" className="text-sm text-red-700 dark:text-red-400">
          <span aria-hidden="true">⚠ </span>
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <button type="submit" disabled={busy} className={danger ? dangerButton : primaryButton}>
          {busy ? "Working…" : submit}
        </button>
        <button type="button" disabled={busy} onClick={onCancel} className={secondaryButton}>
          Cancel
        </button>
      </div>
    </>
  );
}
