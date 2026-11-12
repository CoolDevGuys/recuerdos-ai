import { useId, useState, type FormEvent } from "react";
import type { ApiClient } from "../api/client";
import type { Topic } from "../api/types";
import { memoriesCount } from "../domain/categories";
import { useTopics, type MemoryActions } from "../state/memories";
import { useToasts } from "../state/toasts";
import { field, hint, label, primaryButton, secondaryButton } from "./styles";

const TOPIC_ID = /^[a-z][a-z0-9_-]{0,31}$/;

/**
 * What memories are about (ai-specs/memory-structure-plan.md §3.3): the
 * built-in topics and the user's own, with how many memories each holds.
 * Relabel any (display only: memories store the id), add your own, or open
 * a topic's memories in the list. Moving memories between topics is a bulk
 * edit in the list.
 */
export function TopicsView({
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
  onShowMemories: (topic: string) => void;
}) {
  const topics = useTopics(client, user);
  if (topics.isPending) return <p role="status" className="text-sm text-slate-600 dark:text-slate-400">Loading topics…</p>;
  if (topics.isError) {
    return (
      <p role="alert" className="text-sm text-red-700 dark:text-red-400">
        The topics could not be loaded: {topics.error.message}
      </p>
    );
  }
  return (
    <section aria-label="Topics" className="space-y-4">
      {canWrite && <AddTopic actions={actions} />}
      <ul className="divide-y divide-slate-200 rounded-lg border border-slate-200 bg-white dark:divide-slate-800 dark:border-slate-800 dark:bg-slate-900">
        {topics.data.map((topic) => (
          <TopicRow key={topic.id} topic={topic} canWrite={canWrite} actions={actions} onShowMemories={onShowMemories} />
        ))}
      </ul>
    </section>
  );
}

function TopicRow({
  topic,
  canWrite,
  actions,
  onShowMemories,
}: {
  topic: Topic;
  canWrite: boolean;
  actions: MemoryActions;
  onShowMemories: (topic: string) => void;
}) {
  const id = useId();
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(topic.label);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async (next: string | null) => {
    setBusy(true);
    const outcome = await actions.relabelTopic(topic.id, next);
    setBusy(false);
    if (outcome.kind === "failed") return setError(outcome.message);
    setEditing(false);
    setError(null);
  };

  return (
    <li className="px-4 py-3">
      {editing ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void save(value);
          }}
          className="space-y-2"
          noValidate
        >
          <label htmlFor={`${id}-label`} className={label}>
            Label for “{topic.id}”
          </label>
          <input id={`${id}-label`} value={value} onChange={(event) => setValue(event.target.value)} className={field} />
          <p className={hint}>Only what is shown changes: memories keep the topic.</p>
          {error !== null && (
            <p role="alert" className="text-sm text-red-700 dark:text-red-400">
              {error}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={busy} className={primaryButton}>
              Save label
            </button>
            {topic.label !== topic.default_label && (
              <button type="button" disabled={busy} onClick={() => void save(null)} className={secondaryButton}>
                Back to “{topic.default_label}”
              </button>
            )}
            <button type="button" disabled={busy} onClick={() => setEditing(false)} className={secondaryButton}>
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <span className="flex-1 text-sm text-slate-900 dark:text-slate-100">
            <span className="font-medium">{topic.label}</span>
            {!topic.built_in && <span className="ml-2 text-xs text-slate-500">yours</span>}
            {topic.label !== topic.default_label && <span className="ml-2 text-xs text-slate-500">({topic.id})</span>}
          </span>
          <span className="text-sm text-slate-600 dark:text-slate-400">{memoriesCount(topic.count)}</span>
          {topic.count > 0 && (
            <button type="button" onClick={() => onShowMemories(topic.id)} className="text-sm font-medium text-indigo-700 underline dark:text-indigo-300">
              Show
            </button>
          )}
          {canWrite && (
            <button
              type="button"
              aria-label={`Relabel ${topic.label}`}
              onClick={() => {
                setValue(topic.label);
                setEditing(true);
              }}
              className="text-sm font-medium text-indigo-700 underline dark:text-indigo-300"
            >
              Relabel
            </button>
          )}
        </div>
      )}
    </li>
  );
}

function AddTopic({ actions }: { actions: MemoryActions }) {
  const id = useId();
  const { show } = useToasts();
  const [topicId, setTopicId] = useState("");
  const [topicLabel, setTopicLabel] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const normalised = topicId.trim().toLowerCase();
    if (!TOPIC_ID.test(normalised)) {
      return setError("Start with a letter and use only a–z, 0–9, “-” and “_”, up to 32 characters, like “homelab”.");
    }
    setBusy(true);
    const outcome = await actions.createTopic(normalised, topicLabel.trim() === "" ? null : topicLabel.trim());
    setBusy(false);
    if (outcome.kind === "failed") return setError(outcome.message);
    show({ tone: "success", message: `Added the topic ${outcome.value.label}.` });
    setTopicId("");
    setTopicLabel("");
    setError(null);
  };

  return (
    <form onSubmit={(event) => void submit(event)} className="grid gap-3 rounded-lg border border-slate-200 p-4 sm:grid-cols-[1fr_1fr_auto] dark:border-slate-800" noValidate>
      <div>
        <label htmlFor={`${id}-id`} className={label}>
          New topic
        </label>
        <input
          id={`${id}-id`}
          value={topicId}
          onChange={(event) => {
            setTopicId(event.target.value);
            setError(null);
          }}
          placeholder="homelab"
          aria-invalid={error !== null}
          className={field}
        />
      </div>
      <div>
        <label htmlFor={`${id}-label`} className={label}>
          Label (optional)
        </label>
        <input id={`${id}-label`} value={topicLabel} onChange={(event) => setTopicLabel(event.target.value)} placeholder="Home lab" className={field} />
      </div>
      <div className="self-end">
        <button type="submit" disabled={busy} className={primaryButton}>
          Add topic
        </button>
      </div>
      {error !== null && (
        <p role="alert" className="text-sm text-red-700 sm:col-span-3 dark:text-red-400">
          {error}
        </p>
      )}
    </form>
  );
}
