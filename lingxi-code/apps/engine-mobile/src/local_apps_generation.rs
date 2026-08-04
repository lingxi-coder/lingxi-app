//! Concrete mobile executor for the fixed local-app generation pipeline.

use crate::local_apps_host::LocalAppsHostBroker;
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{AppEventDto, AppGenerationJobDto, AppGenerationJobStateDto};
use local_apps::{
    load_manifest, save_manifest, AppDataStore, AppError, AppGenerationExecutor, AppLayout,
    AppManifest, AppService, DataCollectionSchema, DataFieldKind, DataFieldSchema, DesignValue,
    GenerationJob, GenerationJobObserver, GenerationJobStatus, GenerationRequest,
    GenerationRequestKind, WorkspaceSourcePolicy,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use traits::{LinuxCommandRequest, MobileLinuxRuntime, MountPurpose, MountSpec, NetworkPolicy};

const BUILD_TIMEOUT_MS: u64 = 180_000;

const LOCKED_FILES: &[(&str, &[u8])] = &[
    (
        "package.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/package.json"
        )),
    ),
    (
        "package-lock.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/package-lock.json"
        )),
    ),
    (
        "next.config.mjs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/next.config.mjs"
        )),
    ),
];

const SOURCE_FILES: &[(&str, &[u8])] = &[
    (
        "app/layout.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/app/layout.jsx"
        )),
    ),
    (
        "app/page.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/app/page.jsx"
        )),
    ),
    (
        "app/globals.css",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/app/globals.css"
        )),
    ),
    (
        "components/AppShell.jsx",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/components/AppShell.jsx"
        )),
    ),
    (
        "lib/lingxi-bridge.js",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../local-apps/templates/next-static-v1/lib/lingxi-bridge.js"
        )),
    ),
    ("public/.gitkeep", b""),
];

const APP_SHELL_TEMPLATE: &str = r####""use client";

import { useEffect, useMemo, useState } from "react";
import {
  getLingXiBridge,
  mutateCollection,
  queryCollection,
  requestNetwork,
  requestRuntimeStatus,
} from "../lib/lingxi-bridge";

const APP_NAME = __APP_NAME__;
const TEMPLATE = __TEMPLATE__;
const DESIGN = __SPEC__;
const TEMPLATE_LABEL = TEMPLATE.replaceAll("_", " ");
const COLLECTION_ID = {
  dashboard: "records",
  crud_tracker: "items",
  content_showcase: "entries",
  form_utility: "submissions",
}[TEMPLATE] ?? "records";
const COLLECTION_FIELDS = readFieldList("collection_fields");
const PRIMARY_COLOR = readScalar("primary_color", "#3366FF");
const DENSITY = readScalar("density", "comfortable");
const PAGES = readList("pages");
const FEATURES = readList("features");
const NETWORK_DOMAINS = readList("network_domains");
const DASHBOARD_REFRESH = readScalar("refresh_policy", "manual");
const CRUD_ENTITY_NAME = readScalar("entity_name", "Item");
const CRUD_ALLOW_ARCHIVE = readBoolean("allow_archive", true);
const CRUD_STATUS_FIELD =
  COLLECTION_FIELDS.find((field) => field.id === "status") ??
  COLLECTION_FIELDS.find(
    (field) => field.kind === "enum" && Array.isArray(field.options) && field.options.length
  ) ??
  null;
const CONTENT_TYPE = readScalar("content_type", "Entry");
const CONTENT_PRESENTATION = readScalar("presentation", "grid");
const CONTENT_SEARCH_ENABLED = readBoolean("enable_search", true);
const CONTENT_CATEGORY_SUGGESTIONS = readList("categories");
const FORM_BEHAVIOR = readScalar("behavior", "save");
const FORM_RESULT_DESCRIPTION = readScalar(
  "result_description",
  "Describe the result shown after submission."
);
const FORM_HISTORY_ENABLED = readBoolean("save_history", false);
const FORM_INPUT_FIELDS = COLLECTION_FIELDS.filter(
  (field) => field.id !== "result" && field.id !== "submitted_at"
);

export function AppShell() {
  const [bridgeReady, setBridgeReady] = useState(false);
  const [records, setRecords] = useState([]);
  const [loadState, setLoadState] = useState({ status: "idle", error: "" });
  const [runtimeState, setRuntimeState] = useState({
    status: "idle",
    payload: null,
    error: "",
  });
  const [networkState, setNetworkState] = useState({
    status: "idle",
    message: "",
  });
  const [crudSearch, setCrudSearch] = useState("");
  const [crudStatus, setCrudStatus] = useState("all");
  const [selectedRecordId, setSelectedRecordId] = useState("");
  const [formDraft, setFormDraft] = useState(() =>
    createEmptyFormState(activeEditorFields())
  );
  const [formMessage, setFormMessage] = useState("");
  const [formResult, setFormResult] = useState("");
  const [contentSearch, setContentSearch] = useState("");
  const [contentCategory, setContentCategory] = useState("all");

  useEffect(() => {
    setBridgeReady(getLingXiBridge() !== null);
  }, []);

  useEffect(() => {
    if (!bridgeReady) {
      return;
    }
    void reloadRecords();
    void reloadRuntime();
  }, [bridgeReady]);

  useEffect(() => {
    if (TEMPLATE !== "crud_tracker") {
      return;
    }
    const currentRecord = records.find((record) => record.recordId === selectedRecordId);
    setFormDraft(
      currentRecord
        ? documentToFormState(currentRecord.document, COLLECTION_FIELDS)
        : createEmptyFormState(COLLECTION_FIELDS)
    );
  }, [records, selectedRecordId]);

  useEffect(() => {
    if (TEMPLATE !== "form_utility") {
      return;
    }
    const latestRecord = records[0];
    if (FORM_HISTORY_ENABLED || !latestRecord) {
      setFormDraft(createEmptyFormState(FORM_INPUT_FIELDS));
      return;
    }
    setFormDraft(documentToFormState(latestRecord.document, FORM_INPUT_FIELDS));
    const persistedResult = latestRecord.document?.result;
    if (persistedResult != null) {
      setFormResult(String(persistedResult));
    }
  }, [records]);

  const dashboardMetrics = useMemo(
    () => buildDashboardMetrics(records, COLLECTION_FIELDS),
    [records]
  );
  const filteredCrudRecords = useMemo(
    () => filterCrudRecords(records, crudSearch, crudStatus),
    [records, crudSearch, crudStatus]
  );
  const contentCategories = useMemo(
    () => collectContentCategories(records, CONTENT_CATEGORY_SUGGESTIONS),
    [records]
  );
  const filteredContentRecords = useMemo(
    () => filterContentRecords(records, contentSearch, contentCategory),
    [records, contentSearch, contentCategory]
  );

  async function reloadRecords() {
    if (!COLLECTION_FIELDS.length) {
      setRecords([]);
      setLoadState({ status: "ready", error: "" });
      return;
    }
    setLoadState({ status: "loading", error: "" });
    try {
      const page = await queryCollection({
        collection: COLLECTION_ID,
        sortKey: { kind: "updated_at" },
        sortDirection: "descending",
        limit: 100,
      });
      setRecords(Array.isArray(page?.records) ? page.records : []);
      setLoadState({ status: "ready", error: "" });
    } catch (error) {
      setLoadState({
        status: "error",
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  async function reloadRuntime() {
    setRuntimeState({ status: "loading", payload: null, error: "" });
    try {
      const payload = await requestRuntimeStatus();
      setRuntimeState({ status: "ready", payload, error: "" });
    } catch (error) {
      setRuntimeState({
        status: "error",
        payload: null,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }

  async function handleNetworkCheck() {
    if (!NETWORK_DOMAINS.length) {
      return;
    }
    setNetworkState({ status: "loading", message: "" });
    try {
      const domain = NETWORK_DOMAINS[0];
      const response = await requestNetwork({
        url: `https://${domain}`,
        method: "GET",
        headers: {
          accept: "application/json, text/plain;q=0.9, */*;q=0.1",
        },
      });
      const status =
        response?.status ??
        response?.status_code ??
        response?.statusCode ??
        "ok";
      setNetworkState({
        status: "ready",
        message: `HTTPS bridge reached ${domain} (${status}).`,
      });
    } catch (error) {
      setNetworkState({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }

  function updateDraftField(fieldId, value) {
    setFormDraft((current) => ({ ...current, [fieldId]: value }));
  }

  async function handleCrudSave(event) {
    event.preventDefault();
    const currentRecord = records.find((record) => record.recordId === selectedRecordId);
    const recordId = selectedRecordId || buildRecordId(CRUD_ENTITY_NAME);
    const document = formStateToDocument(formDraft, COLLECTION_FIELDS);
    await mutateCollection({
      collection: COLLECTION_ID,
      operations: [
        {
          kind: "upsert",
          recordId: recordId,
          document,
          expectedRevision: currentRecord?.revision ?? null,
        },
      ],
    });
    setSelectedRecordId(recordId);
    await reloadRecords();
  }

  async function handleCrudDelete(record) {
    await mutateCollection({
      collection: COLLECTION_ID,
      operations: [
        {
          kind: "delete",
          recordId: record.recordId,
          expectedRevision: record.revision ?? null,
        },
      ],
    });
    if (record.recordId === selectedRecordId) {
      setSelectedRecordId("");
      setFormDraft(createEmptyFormState(COLLECTION_FIELDS));
    }
    await reloadRecords();
  }

  async function handleCrudArchive(record) {
    if (!CRUD_STATUS_FIELD?.options?.includes("archived")) {
      return;
    }
    const nextDocument = {
      ...record.document,
      [CRUD_STATUS_FIELD.id]: "archived",
    };
    await mutateCollection({
      collection: COLLECTION_ID,
      operations: [
        {
          kind: "upsert",
          recordId: record.recordId,
          document: nextDocument,
          expectedRevision: record.revision ?? null,
        },
      ],
    });
    await reloadRecords();
  }

  async function handleFormSubmit(event) {
    event.preventDefault();
    const document = formStateToDocument(formDraft, FORM_INPUT_FIELDS);
    const computedResult = buildFormResult(document);
    if (hasField(COLLECTION_FIELDS, "result")) {
      document.result = computedResult;
    }
    if (hasField(COLLECTION_FIELDS, "submitted_at")) {
      document.submitted_at = new Date().toISOString();
    }
    const recordId = FORM_HISTORY_ENABLED
      ? buildRecordId("submission")
      : records[0]?.recordId || "latest-submission";
    const expectedRevision = FORM_HISTORY_ENABLED ? null : records[0]?.revision ?? null;
    await mutateCollection({
      collection: COLLECTION_ID,
      operations: [
        {
          kind: "upsert",
          recordId: recordId,
          document,
          expectedRevision: expectedRevision,
        },
      ],
    });
    setFormResult(computedResult);
    setFormMessage(
      FORM_HISTORY_ENABLED
        ? "Submission saved to history."
        : "Latest submission saved."
    );
    if (!FORM_HISTORY_ENABLED) {
      setFormDraft(documentToFormState(document, FORM_INPUT_FIELDS));
    }
    await reloadRecords();
  }

  async function handleFormDelete(record) {
    await mutateCollection({
      collection: COLLECTION_ID,
      operations: [
        {
          kind: "delete",
          recordId: record.recordId,
          expectedRevision: record.revision ?? null,
        },
      ],
    });
    await reloadRecords();
  }

  function resetActiveForm() {
    const fields = activeEditorFields();
    setFormDraft(createEmptyFormState(fields));
    if (TEMPLATE === "crud_tracker") {
      setSelectedRecordId("");
    }
  }

  function activeEditorFields() {
    return TEMPLATE === "form_utility" ? FORM_INPUT_FIELDS : COLLECTION_FIELDS;
  }

  return (
    <main
      className={`app-shell density-${DENSITY}`}
      data-template={TEMPLATE}
      style={{ "--accent": PRIMARY_COLOR }}
    >
      <div className="app-frame">
        <header className="app-header">
          <div>
            <p className="eyebrow">LingXi Local App · {TEMPLATE_LABEL}</p>
            <h1 id="app-title">{APP_NAME}</h1>
            <p className="lede">{summarizePurpose()}</p>
          </div>
          <div className="status-stack" role="status" aria-live="polite">
            <span className="status-pill" data-ready={bridgeReady}>
              {bridgeReady ? "Bridge connected" : "Waiting for bridge"}
            </span>
            <span className="status-pill" data-ready={loadState.status === "ready"}>
              {loadState.status === "loading"
                ? "Loading records"
                : `${records.length} records loaded`}
            </span>
            <span className="status-pill" data-ready={runtimeState.status === "ready"}>
              {runtimeState.status === "error"
                ? "Runtime unavailable"
                : "Runtime status ready"}
            </span>
          </div>
        </header>

        <section className="meta-strip" aria-label="Configured pages and features">
          <MetaList title="Pages" items={PAGES} emptyLabel="No pages configured" />
          <MetaList title="Features" items={FEATURES} emptyLabel="No extra features configured" />
        </section>

        {loadState.status === "error" ? (
          <section className="section-card danger" role="alert">
            <h2>Record bridge error</h2>
            <p>{loadState.error}</p>
          </section>
        ) : null}

        {renderTemplateView({
          records,
          dashboardMetrics,
          filteredCrudRecords,
          filteredContentRecords,
          contentCategories,
          crudSearch,
          crudStatus,
          selectedRecordId,
          formDraft,
          formMessage,
          formResult,
          contentSearch,
          contentCategory,
          runtimeState,
          onCrudSearchChange: setCrudSearch,
          onCrudStatusChange: setCrudStatus,
          onRecordSelect: setSelectedRecordId,
          onDraftFieldChange: updateDraftField,
          onCrudSave: handleCrudSave,
          onCrudDelete: handleCrudDelete,
          onCrudArchive: handleCrudArchive,
          onFormSubmit: handleFormSubmit,
          onFormDelete: handleFormDelete,
          onReset: resetActiveForm,
          onContentSearchChange: setContentSearch,
          onContentCategoryChange: setContentCategory,
          onReloadRecords: reloadRecords,
          onReloadRuntime: reloadRuntime,
        })}

        <section className="section-card network-panel">
          <div className="section-heading">
            <div>
              <h2>Configured bridge access</h2>
              <p>All runtime behavior stays inside the versioned LingXi bridge.</p>
            </div>
            <button
              id="app-network-check"
              type="button"
              className="secondary-button"
              onClick={handleNetworkCheck}
              disabled={!NETWORK_DOMAINS.length || networkState.status === "loading"}
              aria-label="Check configured network domain"
            >
              {networkState.status === "loading" ? "Checking…" : "Check network bridge"}
            </button>
          </div>
          <dl className="definition-grid">
            <div>
              <dt>Collection</dt>
              <dd>{COLLECTION_ID}</dd>
            </div>
            <div>
              <dt>Fields</dt>
              <dd>{COLLECTION_FIELDS.length || 0}</dd>
            </div>
            <div>
              <dt>Domains</dt>
              <dd>{NETWORK_DOMAINS.join(", ") || "None declared"}</dd>
            </div>
          </dl>
          {networkState.message ? (
            <p
              className={`status-copy ${networkState.status === "error" ? "danger-text" : ""}`}
            >
              {networkState.message}
            </p>
          ) : null}
        </section>
      </div>
    </main>
  );
}

function renderTemplateView(props) {
  switch (TEMPLATE) {
    case "dashboard":
      return <DashboardView {...props} />;
    case "crud_tracker":
      return <CrudTrackerView {...props} />;
    case "content_showcase":
      return <ContentShowcaseView {...props} />;
    case "form_utility":
      return <FormUtilityView {...props} />;
    default:
      return (
        <section className="section-card">
          <h2>Unsupported template</h2>
          <p>{TEMPLATE}</p>
        </section>
      );
  }
}

function DashboardView({ records, dashboardMetrics, runtimeState, onReloadRecords, onReloadRuntime }) {
  const recentFields = COLLECTION_FIELDS.slice(0, 3);

  return (
    <section className="stack-layout">
      <div className="section-card">
        <div className="section-heading">
          <div>
            <h2>Dashboard overview</h2>
            <p>Refresh metrics and inspect the latest records from the native store.</p>
          </div>
          <div className="toolbar">
            <button
              id="dashboard-refresh"
              type="button"
              className="primary-button"
              onClick={onReloadRecords}
              aria-label="Refresh dashboard records"
            >
              Refresh data
            </button>
            <button
              id="dashboard-runtime-refresh"
              type="button"
              className="secondary-button"
              onClick={onReloadRuntime}
              aria-label="Refresh runtime status"
            >
              Refresh runtime
            </button>
          </div>
        </div>

        <div className="metric-grid" role="list" aria-label="Dashboard metrics">
          {dashboardMetrics.map((metric) => (
            <article key={metric.label} className="metric-card" role="listitem">
              <p>{metric.label}</p>
              <strong>{metric.value}</strong>
              <span>{metric.detail}</span>
            </article>
          ))}
        </div>
      </div>

      <div className="section-card">
        <div className="section-heading">
          <div>
            <h2>Runtime status</h2>
            <p>
              {DASHBOARD_REFRESH === "on_open"
                ? "This dashboard is configured to refresh on open."
                : "This dashboard refreshes when you explicitly request it."}
            </p>
          </div>
        </div>
        {runtimeState.status === "error" ? (
          <p className="danger-text">{runtimeState.error}</p>
        ) : (
          <pre className="runtime-panel" id="dashboard-runtime-status">
            {JSON.stringify(runtimeState.payload ?? { status: "pending" }, null, 2)}
          </pre>
        )}
      </div>

      <div className="section-card">
        <div className="section-heading">
          <div>
            <h2>Recent records</h2>
            <p>Latest values in the declared collection.</p>
          </div>
        </div>
        {records.length ? (
          <div className="table-wrap">
            <table id="dashboard-record-table">
              <thead>
                <tr>
                  <th scope="col">Record</th>
                  {recentFields.map((field) => (
                    <th key={field.id} scope="col">
                      {field.label}
                    </th>
                  ))}
                  <th scope="col">Updated</th>
                </tr>
              </thead>
              <tbody>
                {records.slice(0, 6).map((record) => (
                  <tr key={record.recordId}>
                    <th scope="row">{record.recordId}</th>
                    {recentFields.map((field) => (
                      <td key={field.id}>{presentFieldValue(field, record.document?.[field.id])}</td>
                    ))}
                    <td>{formatTimestamp(record.updatedAtMs)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="empty-state">No records yet. Add records through the app data bridge.</p>
        )}
      </div>
    </section>
  );
}

function CrudTrackerView({
  filteredCrudRecords,
  crudSearch,
  crudStatus,
  selectedRecordId,
  formDraft,
  onCrudSearchChange,
  onCrudStatusChange,
  onRecordSelect,
  onDraftFieldChange,
  onCrudSave,
  onCrudDelete,
  onCrudArchive,
  onReset,
}) {
  return (
    <section className="split-layout">
      <div className="section-card">
        <div className="section-heading">
          <div>
            <h2>{CRUD_ENTITY_NAME} records</h2>
            <p>Query, filter, edit, archive, and delete through the native collection bridge.</p>
          </div>
          <button
            id="crud-new-record"
            type="button"
            className="secondary-button"
            onClick={onReset}
            aria-label={`Create a new ${CRUD_ENTITY_NAME}`}
          >
            New {CRUD_ENTITY_NAME}
          </button>
        </div>

        <div className="toolbar" role="search">
          <label className="field">
            <span>Search</span>
            <input
              id="crud-search-input"
              type="search"
              value={crudSearch}
              onChange={(event) => onCrudSearchChange(event.target.value)}
              placeholder={`Search ${CRUD_ENTITY_NAME.toLowerCase()}s`}
            />
          </label>
          {CRUD_STATUS_FIELD ? (
            <label className="field">
              <span>Status</span>
              <select
                id="crud-status-filter"
                value={crudStatus}
                onChange={(event) => onCrudStatusChange(event.target.value)}
              >
                <option value="all">All statuses</option>
                {CRUD_STATUS_FIELD.options.map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
        </div>

        <ul id="crud-record-list" className="record-list" aria-label={`${CRUD_ENTITY_NAME} records`}>
          {filteredCrudRecords.map((record) => (
            <li key={record.recordId} className="record-item">
              <button
                type="button"
                className={`record-select ${record.recordId === selectedRecordId ? "active" : ""}`}
                onClick={() => onRecordSelect(record.recordId)}
                aria-label={`Edit ${recordSummary(record)}`}
              >
                <strong>{recordSummary(record)}</strong>
                <span>{formatTimestamp(record.updatedAtMs)}</span>
              </button>
              <div className="record-actions">
                {CRUD_ALLOW_ARCHIVE && CRUD_STATUS_FIELD?.options?.includes("archived") ? (
                  <button
                    id={`crud-archive-${record.recordId}`}
                    type="button"
                    className="ghost-button"
                    onClick={() => onCrudArchive(record)}
                    aria-label={`Archive ${recordSummary(record)}`}
                  >
                    Archive
                  </button>
                ) : null}
                <button
                  id={`crud-delete-${record.recordId}`}
                  type="button"
                  className="ghost-button danger-button"
                  onClick={() => onCrudDelete(record)}
                  aria-label={`Delete ${recordSummary(record)}`}
                >
                  Delete
                </button>
              </div>
            </li>
          ))}
        </ul>

        {!filteredCrudRecords.length ? (
          <p className="empty-state">No matching records. Create one from the editor panel.</p>
        ) : null}
      </div>

      <form className="section-card" onSubmit={onCrudSave}>
        <div className="section-heading">
          <div>
            <h2>{selectedRecordId ? `Edit ${CRUD_ENTITY_NAME}` : `New ${CRUD_ENTITY_NAME}`}</h2>
            <p>Every save becomes a structured bridge mutation with optimistic revision checks.</p>
          </div>
        </div>
        <div className="field-grid">
          {COLLECTION_FIELDS.map((field) => (
            <FieldEditor
              key={field.id}
              field={field}
              prefix="crud"
              value={formDraft[field.id] ?? emptyFieldValue(field)}
              onChange={(value) => onDraftFieldChange(field.id, value)}
            />
          ))}
        </div>
        <div className="toolbar">
          <button
            id="crud-save-record"
            type="submit"
            className="primary-button"
            aria-label={`Save ${CRUD_ENTITY_NAME}`}
          >
            Save {CRUD_ENTITY_NAME}
          </button>
          <button
            id="crud-reset-editor"
            type="button"
            className="secondary-button"
            onClick={onReset}
            aria-label="Reset CRUD editor"
          >
            Reset
          </button>
        </div>
      </form>
    </section>
  );
}

function ContentShowcaseView({
  filteredContentRecords,
  contentCategories,
  contentSearch,
  contentCategory,
  onContentSearchChange,
  onContentCategoryChange,
}) {
  return (
    <section className="stack-layout">
      <div className="section-card">
        <div className="section-heading">
          <div>
            <h2>{CONTENT_TYPE} showcase</h2>
            <p>Browse the declared collection in a searchable {CONTENT_PRESENTATION} presentation.</p>
          </div>
        </div>
        <div className="toolbar">
          {CONTENT_SEARCH_ENABLED ? (
            <label className="field grow">
              <span>Search</span>
              <input
                id="content-search-input"
                type="search"
                value={contentSearch}
                onChange={(event) => onContentSearchChange(event.target.value)}
                placeholder={`Search ${CONTENT_TYPE.toLowerCase()} content`}
              />
            </label>
          ) : null}
          <label className="field">
            <span>Category</span>
            <select
              id="content-category-filter"
              value={contentCategory}
              onChange={(event) => onContentCategoryChange(event.target.value)}
            >
              <option value="all">All categories</option>
              {contentCategories.map((category) => (
                <option key={category} value={category}>
                  {category}
                </option>
              ))}
            </select>
          </label>
        </div>
      </div>

      <div
        id="content-results"
        className={CONTENT_PRESENTATION === "grid" ? "content-grid" : "content-list"}
        aria-label={`${CONTENT_TYPE} results`}
      >
        {filteredContentRecords.map((record) => {
          const titleField = COLLECTION_FIELDS.find((field) => field.id === "title") ?? COLLECTION_FIELDS[0];
          const categoryField = COLLECTION_FIELDS.find((field) => field.id === "category");
          const bodyField = COLLECTION_FIELDS.find((field) => field.id === "body");
          const imageField = COLLECTION_FIELDS.find((field) => field.id === "image");
          const title = presentFieldValue(titleField, record.document?.[titleField?.id]);
          const category = categoryField ? presentFieldValue(categoryField, record.document?.[categoryField.id]) : "Uncategorized";
          const body = bodyField ? presentFieldValue(bodyField, record.document?.[bodyField.id]) : "";
          const image = imageField ? record.document?.[imageField.id] : "";

          return (
            <article key={record.recordId} className="content-card">
              {typeof image === "string" && image ? (
                <img
                  className="content-image"
                  src={image}
                  alt={`${title} image`}
                />
              ) : null}
              <div className="content-copy">
                <p className="pill">{category || "Uncategorized"}</p>
                <h3>{title}</h3>
                <p>{body || "No body content has been saved for this entry yet."}</p>
              </div>
            </article>
          );
        })}
      </div>

      {!filteredContentRecords.length ? (
        <section className="section-card">
          <p className="empty-state">No content matches the current filters.</p>
        </section>
      ) : null}
    </section>
  );
}

function FormUtilityView({
  records,
  formDraft,
  formMessage,
  formResult,
  onDraftFieldChange,
  onFormSubmit,
  onFormDelete,
  onReset,
}) {
  return (
    <section className="split-layout">
      <form className="section-card" onSubmit={onFormSubmit}>
        <div className="section-heading">
          <div>
            <h2>Form utility</h2>
            <p>
              {FORM_BEHAVIOR === "calculate"
                ? "Calculate a result from the submitted values."
                : FORM_BEHAVIOR === "generate"
                  ? "Generate a formatted result from the submitted values."
                  : "Save a result and keep the latest submission available in-app."}
            </p>
          </div>
        </div>
        <div className="field-grid">
          {FORM_INPUT_FIELDS.map((field) => (
            <FieldEditor
              key={field.id}
              field={field}
              prefix="form"
              value={formDraft[field.id] ?? emptyFieldValue(field)}
              onChange={(value) => onDraftFieldChange(field.id, value)}
            />
          ))}
        </div>
        <div className="toolbar">
          <button
            id="form-submit"
            type="submit"
            className="primary-button"
            aria-label="Submit form utility"
          >
            {FORM_BEHAVIOR === "save" ? "Save result" : "Run utility"}
          </button>
          <button
            id="form-reset"
            type="button"
            className="secondary-button"
            onClick={onReset}
            aria-label="Reset form utility"
          >
            Reset
          </button>
        </div>
        {formMessage ? <p className="status-copy">{formMessage}</p> : null}
      </form>

      <div className="stack-layout">
        <section className="section-card result-panel">
          <div className="section-heading">
            <div>
              <h2>Computed result</h2>
              <p>{FORM_RESULT_DESCRIPTION}</p>
            </div>
          </div>
          <output id="form-result-output">{formResult || "Submit the form to compute a result."}</output>
        </section>

        <section className="section-card">
          <div className="section-heading">
            <div>
              <h2>Saved submissions</h2>
              <p>{FORM_HISTORY_ENABLED ? "Each submission is kept as a separate record." : "The latest submission is kept in place for fast editing."}</p>
            </div>
          </div>
          <ul id="form-history" className="record-list" aria-label="Saved submissions">
            {records.map((record) => (
              <li key={record.recordId} className="record-item">
                <div>
                  <strong>{record.recordId}</strong>
                  <span>{formatTimestamp(record.updatedAtMs)}</span>
                </div>
                <button
                  id={`form-delete-${record.recordId}`}
                  type="button"
                  className="ghost-button danger-button"
                  onClick={() => onFormDelete(record)}
                  aria-label={`Delete submission ${record.recordId}`}
                >
                  Delete
                </button>
              </li>
            ))}
          </ul>
          {!records.length ? (
            <p className="empty-state">No submissions saved yet.</p>
          ) : null}
        </section>
      </div>
    </section>
  );
}

function MetaList({ title, items, emptyLabel }) {
  return (
    <div className="meta-group">
      <span>{title}</span>
      <div className="pill-row">
        {items.length ? items.map((item) => <span key={item} className="pill">{item}</span>) : <span className="subtle-copy">{emptyLabel}</span>}
      </div>
    </div>
  );
}

function FieldEditor({ field, prefix, value, onChange }) {
  const fieldId = `${prefix}-field-${field.id}`;

  if (field.kind === "long_text") {
    return (
      <label className="field wide" htmlFor={fieldId}>
        <span>{field.label}</span>
        <textarea
          id={fieldId}
          rows={4}
          required={field.required}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      </label>
    );
  }

  if (field.kind === "boolean") {
    return (
      <label className="toggle-field" htmlFor={fieldId}>
        <input
          id={fieldId}
          type="checkbox"
          checked={Boolean(value)}
          onChange={(event) => onChange(event.target.checked)}
        />
        <span>{field.label}</span>
      </label>
    );
  }

  if (field.kind === "enum") {
    return (
      <label className="field" htmlFor={fieldId}>
        <span>{field.label}</span>
        <select
          id={fieldId}
          required={field.required}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          {!field.required ? <option value="">Select one</option> : null}
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </label>
    );
  }

  return (
    <label className="field" htmlFor={fieldId}>
      <span>{field.label}</span>
      <input
        id={fieldId}
        type={inputType(field.kind)}
        required={field.required}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        step={field.kind === "decimal" ? "0.01" : undefined}
      />
    </label>
  );
}

function buildDashboardMetrics(records, fields) {
  const numericField = fields.find((field) => field.kind === "integer" || field.kind === "decimal");
  const dateField = fields.find((field) => field.kind === "date_time");
  const numericTotal = numericField
    ? records.reduce((total, record) => total + Number(record.document?.[numericField.id] ?? 0), 0)
    : null;
  const latestValue = dateField
    ? records
        .map((record) => record.document?.[dateField.id])
        .filter(Boolean)
        .sort()
        .at(-1)
    : null;

  return [
    {
      label: "Total records",
      value: String(records.length),
      detail: "Counted through queryCollection",
    },
    {
      label: numericField ? numericField.label : "Configured fields",
      value: numericField ? formatNumber(numericTotal) : String(fields.length),
      detail: numericField ? "Summed from numeric records" : "Declared collection fields",
    },
    {
      label: dateField ? dateField.label : "Last updated",
      value: dateField && latestValue ? String(latestValue) : formatTimestamp(records[0]?.updatedAtMs),
      detail: dateField ? "Latest timestamp in the collection" : "Newest stored record",
    },
  ];
}

function filterCrudRecords(records, search, status) {
  return records.filter((record) => {
    if (status !== "all" && CRUD_STATUS_FIELD) {
      if (String(record.document?.[CRUD_STATUS_FIELD.id] ?? "") !== status) {
        return false;
      }
    }
    if (!search.trim()) {
      return true;
    }
    const haystack = JSON.stringify(record.document ?? {}).toLowerCase();
    return haystack.includes(search.trim().toLowerCase());
  });
}

function collectContentCategories(records, suggestions) {
  const values = new Set(suggestions.filter(Boolean));
  const categoryField = COLLECTION_FIELDS.find((field) => field.id === "category");
  if (categoryField) {
    records.forEach((record) => {
      const value = record.document?.[categoryField.id];
      if (typeof value === "string" && value) {
        values.add(value);
      }
    });
  }
  return [...values];
}

function filterContentRecords(records, search, category) {
  return records.filter((record) => {
    if (category !== "all") {
      const recordCategory = String(record.document?.category ?? "");
      if (recordCategory !== category) {
        return false;
      }
    }
    if (!search.trim()) {
      return true;
    }
    return JSON.stringify(record.document ?? {})
      .toLowerCase()
      .includes(search.trim().toLowerCase());
  });
}

function buildFormResult(document) {
  const summary = FORM_INPUT_FIELDS.map((field) => {
    const value = document[field.id];
    if (value == null || value === "") {
      return null;
    }
    return `${field.label}: ${presentFieldValue(field, value)}`;
  }).filter(Boolean);

  if (FORM_BEHAVIOR === "calculate") {
    const total = FORM_INPUT_FIELDS.reduce((sum, field) => {
      if (field.kind !== "integer" && field.kind !== "decimal") {
        return sum;
      }
      const value = Number(document[field.id] ?? 0);
      return Number.isFinite(value) ? sum + value : sum;
    }, 0);
    return `${FORM_RESULT_DESCRIPTION} Total: ${formatNumber(total)}.`;
  }

  if (FORM_BEHAVIOR === "generate") {
    return [FORM_RESULT_DESCRIPTION, ...summary].filter(Boolean).join("\n");
  }

  const timestamp = new Date().toLocaleString();
  return summary.length
    ? `${FORM_RESULT_DESCRIPTION} Saved at ${timestamp}: ${summary.join(" • ")}`
    : `${FORM_RESULT_DESCRIPTION} Saved at ${timestamp}.`;
}

function summarizePurpose() {
  const purpose = unwrapDesignValue(DESIGN.purpose);
  if (typeof purpose === "string" && purpose.trim()) {
    return purpose;
  }
  return "This static-export app renders data, runtime, and optional network checks exclusively through the LingXi bridge.";
}

function recordSummary(record) {
  const titleField =
    COLLECTION_FIELDS.find((field) => field.id === "title" || field.id === "label") ??
    COLLECTION_FIELDS[0];
  const label = titleField
    ? presentFieldValue(titleField, record.document?.[titleField.id])
    : record.recordId;
  return label || record.recordId;
}

function inputType(kind) {
  switch (kind) {
    case "integer":
    case "decimal":
      return "number";
    case "date_time":
      return "datetime-local";
    default:
      return "text";
  }
}

function emptyFieldValue(field) {
  switch (field.kind) {
    case "boolean":
      return false;
    default:
      return "";
  }
}

function createEmptyFormState(fields) {
  return Object.fromEntries(fields.map((field) => [field.id, emptyFieldValue(field)]));
}

function documentToFormState(document, fields) {
  return Object.fromEntries(
    fields.map((field) => {
      const value = document?.[field.id];
      if (value == null) {
        return [field.id, emptyFieldValue(field)];
      }
      if (field.kind === "date_time" && typeof value === "string") {
        return [field.id, value.slice(0, 16)];
      }
      return [field.id, value];
    })
  );
}

function formStateToDocument(state, fields) {
  return Object.fromEntries(
    fields.map((field) => [field.id, normalizeFieldValue(field, state[field.id])])
  );
}

function normalizeFieldValue(field, rawValue) {
  if (field.kind === "boolean") {
    return Boolean(rawValue);
  }
  if (field.kind === "integer") {
    const parsed = Number.parseInt(String(rawValue || "0"), 10);
    return Number.isFinite(parsed) ? parsed : 0;
  }
  if (field.kind === "decimal") {
    const parsed = Number.parseFloat(String(rawValue || "0"));
    return Number.isFinite(parsed) ? parsed : 0;
  }
  if (field.kind === "date_time") {
    if (!rawValue) {
      return "";
    }
    const date = new Date(rawValue);
    return Number.isNaN(date.valueOf()) ? String(rawValue) : date.toISOString();
  }
  return String(rawValue ?? "");
}

function presentFieldValue(field, value) {
  if (value == null || value === "") {
    return "—";
  }
  if (field?.kind === "boolean") {
    return value ? "Yes" : "No";
  }
  if (field?.kind === "date_time") {
    return formatTimestamp(Date.parse(value));
  }
  return String(value);
}

function formatTimestamp(value) {
  if (!value) {
    return "—";
  }
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) {
    return String(value);
  }
  return date.toLocaleString();
}

function formatNumber(value) {
  if (value == null || Number.isNaN(value)) {
    return "—";
  }
  return new Intl.NumberFormat().format(value);
}

function buildRecordId(prefix) {
  return `${slugify(prefix)}-${Date.now().toString(36)}`;
}

function slugify(value) {
  return String(value || "record")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "record";
}

function hasField(fields, fieldId) {
  return fields.some((field) => field.id === fieldId);
}

function readDesignValue(fieldId) {
  return DESIGN[fieldId];
}

function unwrapDesignValue(value) {
  if (!value || typeof value !== "object") {
    return value;
  }
  if ("value" in value) {
    return value.value;
  }
  return value;
}

function readScalar(fieldId, fallback) {
  const value = unwrapDesignValue(readDesignValue(fieldId));
  return typeof value === "string" && value.length ? value : fallback;
}

function readBoolean(fieldId, fallback) {
  const value = unwrapDesignValue(readDesignValue(fieldId));
  return typeof value === "boolean" ? value : fallback;
}

function readList(fieldId) {
  const value = unwrapDesignValue(readDesignValue(fieldId));
  return Array.isArray(value) ? value.filter(Boolean).map(String) : [];
}

function readFieldList(fieldId) {
  const value = unwrapDesignValue(readDesignValue(fieldId));
  if (!Array.isArray(value)) {
    return [];
  }
  return value.map((field) => ({
    id: String(field.id),
    label: String(field.label),
    kind: String(field.kind ?? field.field_type ?? "text"),
    required: Boolean(field.required),
    options: Array.isArray(field.enumOptions)
      ? field.enumOptions.map(String)
      : Array.isArray(field.options)
        ? field.options.map(String)
        : [],
  }));
}
"####;

pub(crate) struct MobileAppGenerationExecutor {
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    host: Arc<LocalAppsHostBroker>,
    service: OnceLock<Arc<AppService>>,
}

impl MobileAppGenerationExecutor {
    pub(crate) fn new(
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        host: Arc<LocalAppsHostBroker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mobile_linux,
            host,
            service: OnceLock::new(),
        })
    }

    pub(crate) fn attach_service(&self, service: Arc<AppService>) -> Result<(), Arc<AppService>> {
        self.service.set(service)
    }

    fn service(&self) -> Result<Arc<AppService>, AppError> {
        self.service
            .get()
            .cloned()
            .ok_or_else(|| AppError::Io("generation service is not attached".into()))
    }

    async fn reconcile_manifest(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        let service = self.service()?;
        let record = service.record(&request.key.app_id).await?;
        let draft = service.draft(&request.key.app_id).await?;
        let mut manifest = load_manifest(layout)?;
        manifest.name = record.name;
        manifest.revision = request.key.revision;
        if let Some(DesignValue::DataFieldList(fields)) = draft.fields.get("collection_fields") {
            if manifest.collections.is_empty() {
                manifest.collections.push(DataCollectionSchema {
                    id: match record.template {
                        local_apps::AppTemplateKind::Dashboard => "records",
                        local_apps::AppTemplateKind::CrudTracker => "items",
                        local_apps::AppTemplateKind::ContentShowcase => "entries",
                        local_apps::AppTemplateKind::FormUtility => "submissions",
                    }
                    .into(),
                    name: "App Data".into(),
                    fields: fields.clone(),
                });
            } else if let Some(collection) = manifest.collections.first_mut() {
                collection.fields = fields.clone();
            }
        }
        if let Some(DesignValue::DomainList(domains)) = draft.fields.get("network_domains") {
            manifest.allowed_domains = domains.clone();
        }
        manifest.validate()?;
        migrate_manifest_with_approval(&self.host, layout, &manifest).await?;
        save_manifest(layout, &manifest)
    }

    async fn run_next_build(&self, layout: &AppLayout, full: bool) -> Result<(), AppError> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            AppError::NotYetAvailable(
                "the verified mobile Node runtime is unavailable in this build".into(),
            )
        })?;
        let build_root = layout.root().join(layout.build_rel(full));
        let request = LinuxCommandRequest {
            command: "/usr/bin/node".into(),
            args: vec![
                "/opt/lingxi/local-app-runtime/node_modules/next/dist/bin/next".into(),
                "build".into(),
            ],
            cwd: Some("/workspace".into()),
            env: [
                (
                    "LINGXI_APP_OUTPUT".into(),
                    if full { "server" } else { "export" }.into(),
                ),
                (
                    "NODE_PATH".into(),
                    "/opt/lingxi/local-app-runtime/node_modules".into(),
                ),
            ]
            .into_iter()
            .collect(),
            stdin: None,
            timeout_ms: Some(BUILD_TIMEOUT_MS),
            // See local_apps_host.rs: the shipped mobile runtimes accept only
            // `Allowed` and reject the request outright otherwise.
            network: NetworkPolicy::Allowed,
            mounts: vec![
                MountSpec {
                    host_path: build_root,
                    guest_path: "/workspace".into(),
                    read_only: false,
                    purpose: MountPurpose::Workspace,
                },
                self.host
                    .fixed_runtime_mount()
                    .map_err(AppError::NotYetAvailable)?,
            ],
        };
        let result = runtime
            .run(request)
            .await
            .map_err(|error| AppError::Io(format!("fixed Next build failed: {error}")))?;
        append_build_log(layout, full, &result.stdout, &result.stderr).await?;
        if result.timed_out || result.cancelled || result.exit_code != 0 {
            return Err(AppError::Io(format!(
                "fixed Next build exited {} (timed_out={}, cancelled={}): {}",
                result.exit_code,
                result.timed_out,
                result.cancelled,
                bounded_log(&result.stderr)
            )));
        }
        Ok(())
    }
}

async fn migrate_manifest_with_approval(
    host: &LocalAppsHostBroker,
    layout: &AppLayout,
    manifest: &AppManifest,
) -> Result<(), AppError> {
    let preview = preview_manifest_migration(layout.clone(), manifest.clone()).await?;
    let allow_destructive = if preview.destructive {
        host.approve_destructive_manifest_migration(manifest.app_id.as_str(), &preview)
            .await
            .map_err(AppError::InvalidRequest)?;
        true
    } else {
        false
    };
    apply_manifest_migration(
        layout.clone(),
        manifest.clone(),
        allow_destructive,
        allow_destructive.then_some(preview),
    )
    .await
}

async fn preview_manifest_migration(
    layout: AppLayout,
    manifest: AppManifest,
) -> Result<local_apps::DataMigrationPreview, AppError> {
    tokio::task::spawn_blocking(move || {
        let store = AppDataStore::open(layout)?;
        store.preview_migration(&manifest)
    })
    .await
    .map_err(|error| AppError::Io(format!("manifest migration worker failed: {error}")))?
}

async fn apply_manifest_migration(
    layout: AppLayout,
    manifest: AppManifest,
    allow_destructive: bool,
    approved_preview: Option<local_apps::DataMigrationPreview>,
) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || {
        let mut store = AppDataStore::open(layout)?;
        if let Some(approved_preview) = approved_preview {
            let current_preview = store.preview_migration(&manifest)?;
            if current_preview != approved_preview {
                return Err(AppError::WorkflowStateInvalid(
                    "destructive data migration changed while waiting for approval; retry generation"
                        .into(),
                ));
            }
        }
        store.migrate_manifest(&manifest, allow_destructive, now_ms())?;
        Ok::<_, AppError>(())
    })
    .await
    .map_err(|error| AppError::Io(format!("manifest migration worker failed: {error}")))?
}

async fn append_build_log(
    layout: &AppLayout,
    full: bool,
    stdout: &str,
    stderr: &str,
) -> Result<(), AppError> {
    let log_dir = layout.root().join(layout.logs_rel());
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|error| AppError::Io(format!("create build log directory: {error}")))?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("build.log"))
        .await
        .map_err(|error| AppError::Io(format!("open build log: {error}")))?;
    let channel = if full { "full" } else { "store" };
    let body = format!(
        "\n=== {channel} build ===\nstdout:\n{}\nstderr:\n{}\n",
        bounded_log(stdout),
        bounded_log(stderr)
    );
    file.write_all(body.as_bytes())
        .await
        .map_err(|error| AppError::Io(format!("write build log: {error}")))
}

#[async_trait]
impl AppGenerationExecutor for MobileAppGenerationExecutor {
    async fn prepare_scaffold(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        layout.initialize()?;
        let workspace = layout.root().join(layout.workspace_rel());
        for (relative, bytes) in LOCKED_FILES {
            write_file(&workspace, relative, bytes, true)?;
        }
        for (relative, bytes) in SOURCE_FILES {
            let overwrite = request.kind == GenerationRequestKind::Initial;
            write_file(&workspace, relative, bytes, overwrite)?;
        }
        self.reconcile_manifest(request, layout).await
    }

    async fn generate_source(
        &self,
        request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        if request.kind == GenerationRequestKind::Restore {
            return Ok(());
        }
        let service = self.service()?;
        let record = service.record(&request.key.app_id).await?;
        let draft = service.draft(&request.key.app_id).await?;
        let component = render_app_shell_source(&record.name, record.template, &draft.fields)?;
        let workspace = layout.root().join(layout.workspace_rel());
        write_file(
            &workspace,
            "components/AppShell.jsx",
            component.as_bytes(),
            true,
        )
    }

    async fn source_policy(
        &self,
        _request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<WorkspaceSourcePolicy, AppError> {
        let locked_files = LOCKED_FILES
            .iter()
            .map(|(relative, bytes)| {
                (
                    PathBuf::from(relative),
                    format!("{:x}", Sha256::digest(bytes)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        Ok(WorkspaceSourcePolicy { locked_files })
    }

    async fn validate_source(
        &self,
        _request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<(), AppError> {
        Ok(())
    }

    async fn build(
        &self,
        _request: &GenerationRequest,
        layout: &AppLayout,
    ) -> Result<(), AppError> {
        let workspace = layout.root().join(layout.workspace_rel());
        let build_modes: &[bool] = if self.host.full_runtime_enabled() {
            // Full still proves static-export compatibility before producing
            // the server build used at runtime.
            &[false, true]
        } else {
            &[false]
        };
        for &full in build_modes {
            let build_root = layout.root().join(layout.build_rel(full));
            replace_build_source(&workspace, &build_root)?;
            self.run_next_build(layout, full).await?;
        }
        Ok(())
    }

    async fn start_preview(
        &self,
        request: &GenerationRequest,
        _layout: &AppLayout,
    ) -> Result<Option<String>, AppError> {
        let value = self
            .host
            .manage_runtime_value(serde_json::json!({
                "app_id": request.key.app_id,
                "action": "start",
            }))
            .await
            .map_err(AppError::Io)?;
        Ok(value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}

pub(crate) struct ClientGenerationJobObserver {
    root: PathBuf,
    sink: Arc<dyn ClientEventSink>,
}

impl ClientGenerationJobObserver {
    pub(crate) fn new(root: PathBuf, sink: Arc<dyn ClientEventSink>) -> Arc<Self> {
        Arc::new(Self { root, sink })
    }
}

#[async_trait]
impl GenerationJobObserver for ClientGenerationJobObserver {
    async fn on_job_changed(&self, job: GenerationJob) {
        if let Err(error) = append_generation_log(&self.root, &job).await {
            tracing::warn!(app_id = %job.key.app_id, %error, "failed to append local-app generation log");
        }
        self.sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppGenerationJobChanged {
                    job: lower_job(job),
                },
            })
            .await;
    }
}

async fn append_generation_log(root: &Path, job: &GenerationJob) -> Result<(), AppError> {
    let layout = AppLayout::new(root, &job.key.app_id)?;
    let log_dir = layout.root().join(layout.logs_rel());
    tokio::fs::create_dir_all(&log_dir)
        .await
        .map_err(|error| AppError::Io(format!("create generation log directory: {error}")))?;
    let path = log_dir.join("generation.log");
    if tokio::fs::metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.len() >= 1024 * 1024)
    {
        let _ = tokio::fs::rename(&path, log_dir.join("generation.log.1")).await;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| AppError::Io(format!("open generation log: {error}")))?;
    let line = serde_json::to_string(&serde_json::json!({
        "updatedAtMs": job.updated_at_ms,
        "revision": job.key.revision,
        "continuationSeq": job.key.continuation_seq,
        "attempt": job.attempt,
        "state": job.status,
        "previewUrl": &job.preview_url,
        "error": &job.last_error,
    }))
    .map_err(|error| AppError::Io(format!("serialize generation log: {error}")))?;
    file.write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|error| AppError::Io(format!("write generation log: {error}")))
}

pub(crate) fn lower_job(job: GenerationJob) -> AppGenerationJobDto {
    let (state, percent) = match job.status {
        GenerationJobStatus::Queued => (AppGenerationJobStateDto::Queued, Some(0)),
        GenerationJobStatus::Scaffolding => (AppGenerationJobStateDto::Scaffolding, Some(5)),
        GenerationJobStatus::Generating => (AppGenerationJobStateDto::Generating, Some(25)),
        GenerationJobStatus::Validating => (AppGenerationJobStateDto::Validating, Some(50)),
        GenerationJobStatus::Building => (AppGenerationJobStateDto::Building, Some(70)),
        GenerationJobStatus::StartingPreview => {
            (AppGenerationJobStateDto::StartingPreview, Some(90))
        }
        GenerationJobStatus::AwaitingPreviewApproval => {
            (AppGenerationJobStateDto::AwaitingApproval, Some(100))
        }
        GenerationJobStatus::Completed => (AppGenerationJobStateDto::Succeeded, Some(100)),
        GenerationJobStatus::Failed | GenerationJobStatus::Retryable => {
            (AppGenerationJobStateDto::Failed, None)
        }
    };
    AppGenerationJobDto {
        id: format!(
            "{}:{}:{}",
            job.key.app_id, job.key.revision, job.key.continuation_seq
        ),
        app_id: job.key.app_id,
        revision: job.key.revision,
        continuation_seq: job.key.continuation_seq,
        state,
        percent,
        detail: job.last_error.or(job.preview_url),
        log_rel: Some("logs/generation.log".into()),
        updated_at_ms: job.updated_at_ms,
    }
}

fn write_file(root: &Path, relative: &str, bytes: &[u8], overwrite: bool) -> Result<(), AppError> {
    let path = root.join(relative);
    if !overwrite && path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io(format!("template path {relative} has no parent")))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| AppError::Io(format!("create template directory: {error}")))?;
    std::fs::write(&path, bytes)
        .map_err(|error| AppError::Io(format!("write template file {relative}: {error}")))
}

fn replace_build_source(workspace: &Path, build_root: &Path) -> Result<(), AppError> {
    if build_root.exists() {
        std::fs::remove_dir_all(build_root)
            .map_err(|error| AppError::Io(format!("clear build directory: {error}")))?;
    }
    std::fs::create_dir_all(build_root)
        .map_err(|error| AppError::Io(format!("create build directory: {error}")))?;
    copy_tree(workspace, workspace, build_root)
}

fn copy_tree(workspace: &Path, current: &Path, destination: &Path) -> Result<(), AppError> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| AppError::Io(format!("read generation source: {error}")))?
    {
        let entry = entry.map_err(|error| AppError::Io(format!("read source entry: {error}")))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| AppError::InvalidRequest("build source escaped workspace".into()))?;
        if relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".git")
        {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| AppError::Io(format!("inspect source entry: {error}")))?;
        if kind.is_symlink() {
            return Err(AppError::InvalidRequest(format!(
                "source symlink is forbidden: {}",
                relative.display()
            )));
        }
        let target = destination.join(relative);
        if kind.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| AppError::Io(format!("create build source directory: {error}")))?;
            copy_tree(workspace, &path, destination)?;
        } else if kind.is_file() {
            std::fs::copy(&path, &target)
                .map_err(|error| AppError::Io(format!("copy build source: {error}")))?;
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn bounded_log(value: &str) -> String {
    value.chars().take(4_000).collect()
}

fn render_app_shell_source(
    app_name: &str,
    template: local_apps::AppTemplateKind,
    design_fields: &BTreeMap<String, DesignValue>,
) -> Result<String, AppError> {
    let mut design = design_fields.clone();
    design
        .entry("collection_fields".into())
        .or_insert_with(|| DesignValue::DataFieldList(default_collection_fields(template)));

    let app_name = embed_json_value(
        serde_json::to_value(app_name)
            .map_err(|error| AppError::Io(format!("serialize app name: {error}")))?,
        "app name",
    )?;
    let template = embed_json_value(
        serde_json::to_value(template.as_str())
            .map_err(|error| AppError::Io(format!("serialize template: {error}")))?,
        "template",
    )?;
    let spec = embed_json_value(
        serde_json::to_value(&design)
            .map_err(|error| AppError::Io(format!("serialize design spec: {error}")))?,
        "design spec",
    )?;

    // One pass over the fixed template: chained `replace`s rescan text inserted
    // by an earlier call, so an app name containing `__SPEC__` is expanded into
    // the design object and terminates the emitted JS string literal early.
    let (head, rest) = APP_SHELL_TEMPLATE
        .split_once("__APP_NAME__")
        .expect("app shell template declares __APP_NAME__");
    let (after_name, rest) = rest
        .split_once("__TEMPLATE__")
        .expect("app shell template declares __TEMPLATE__ after __APP_NAME__");
    let (after_template, tail) = rest
        .split_once("__SPEC__")
        .expect("app shell template declares __SPEC__ after __TEMPLATE__");
    Ok(format!(
        "{head}{app_name}{after_name}{template}{after_template}{spec}{tail}"
    ))
}

fn embed_json_value(value: serde_json::Value, label: &str) -> Result<String, AppError> {
    serde_json::to_string(&value)
        .map(|json| json.replace('<', "\\u003c").replace('>', "\\u003e"))
        .map_err(|error| AppError::Io(format!("serialize {label}: {error}")))
}

fn default_collection_fields(template: local_apps::AppTemplateKind) -> Vec<DataFieldSchema> {
    match template {
        local_apps::AppTemplateKind::Dashboard => vec![
            data_field_schema("label", "Label", DataFieldKind::Text, true, &[]),
            data_field_schema("value", "Value", DataFieldKind::Decimal, true, &[]),
            data_field_schema(
                "recorded_at",
                "Recorded at",
                DataFieldKind::DateTime,
                true,
                &[],
            ),
        ],
        local_apps::AppTemplateKind::CrudTracker => vec![
            data_field_schema("title", "Title", DataFieldKind::Text, true, &[]),
            data_field_schema("notes", "Notes", DataFieldKind::LongText, false, &[]),
            data_field_schema(
                "status",
                "Status",
                DataFieldKind::Enum,
                true,
                &["todo", "done"],
            ),
        ],
        local_apps::AppTemplateKind::ContentShowcase => vec![
            data_field_schema("title", "Title", DataFieldKind::Text, true, &[]),
            data_field_schema("category", "Category", DataFieldKind::Text, false, &[]),
            data_field_schema("body", "Body", DataFieldKind::LongText, true, &[]),
            data_field_schema("image", "Image", DataFieldKind::ImageRef, false, &[]),
        ],
        local_apps::AppTemplateKind::FormUtility => vec![
            data_field_schema("input", "Input", DataFieldKind::LongText, true, &[]),
            data_field_schema("result", "Result", DataFieldKind::LongText, false, &[]),
            data_field_schema(
                "submitted_at",
                "Submitted at",
                DataFieldKind::DateTime,
                true,
                &[],
            ),
        ],
    }
}

fn data_field_schema(
    id: &str,
    label: &str,
    kind: DataFieldKind,
    required: bool,
    enum_options: &[&str],
) -> DataFieldSchema {
    DataFieldSchema {
        id: id.into(),
        label: label.into(),
        kind,
        required,
        enum_options: enum_options
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use client_adapter::MockSink;
    use client_protocol::events::ClientEvent;
    use client_protocol::local_apps::{
        AppAuthorizationDecisionDto, AppCapabilityKindDto, AppEventDto,
    };
    use local_apps::{load_permissions, AppTemplateKind};
    use std::sync::Mutex as StdMutex;
    use tokio::time::{sleep, Duration};
    use traits::{
        LinuxCommandResult, LinuxProcessHandle, MobileLinuxCapability, MobileLinuxError,
        MobileLinuxRuntimeMode, MobileLinuxTaskSnapshot, PtyOpenRequest, PtySessionHandle, PtySize,
        RootfsState, RootfsStatus, SandboxBackend,
    };

    /// Captures the one `run` request `run_next_build` issues. Every other
    /// entry point is unreachable from that path and stays `Unsupported`.
    #[derive(Default)]
    struct RecordingMobileLinuxRuntime {
        request: StdMutex<Option<LinuxCommandRequest>>,
    }

    impl RecordingMobileLinuxRuntime {
        fn recorded(&self) -> LinuxCommandRequest {
            self.request
                .lock()
                .expect("recorded request")
                .clone()
                .expect("run was called")
        }

        fn rootfs(&self) -> RootfsStatus {
            RootfsStatus {
                state: RootfsState::Ready,
                backend: self.backend(),
                mode: self.mode(),
                platform: "test".into(),
                abi: "test".into(),
                version: None,
                managed_root: None,
                active_root: None,
                staged_root: None,
                archive_sha256: None,
                installed_size_bytes: None,
                writable_guest_paths: vec![],
                last_error: None,
            }
        }
    }

    #[async_trait]
    impl MobileLinuxRuntime for RecordingMobileLinuxRuntime {
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::IosIsh
        }

        fn mode(&self) -> MobileLinuxRuntimeMode {
            MobileLinuxRuntimeMode::MobileLinux
        }

        async fn probe_capability(&self) -> MobileLinuxCapability {
            MobileLinuxCapability {
                available: true,
                backend: self.backend(),
                mode: self.mode(),
                reason: None,
                streaming_output: false,
                background_processes: true,
                pty: false,
                bind_mounts: true,
                rootfs_integrity: false,
            }
        }

        async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn shutdown(&self) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn run(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<LinuxCommandResult, MobileLinuxError> {
            // The shipped runtimes reject a denied policy before they boot, so
            // record what the caller asked for rather than silently accepting it.
            *self.request.lock().expect("recorded request") = Some(request);
            Ok(LinuxCommandResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                cancelled: false,
            })
        }

        async fn spawn_background(
            &self,
            _request: LinuxCommandRequest,
        ) -> Result<LinuxProcessHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn kill(&self, _handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn open_pty(
            &self,
            _request: PtyOpenRequest,
        ) -> Result<PtySessionHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn write_pty(
            &self,
            _handle: &PtySessionHandle,
            _input: Vec<u8>,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn resize_pty(
            &self,
            _handle: &PtySessionHandle,
            _size: PtySize,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn close_pty(&self, _handle: &PtySessionHandle) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn rootfs_status(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs())
        }

        async fn configure_mounts(&self, _mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            Ok(Vec::new())
        }

        async fn task_status(
            &self,
            _task_id: &str,
        ) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn next_build_request_uses_a_policy_the_mobile_runtimes_accept() {
        let root = tempfile::tempdir().unwrap();
        let runtime_root = root.path().join("runtime-root");
        let next_bin = runtime_root.join("node_modules/next/dist/bin/next");
        std::fs::create_dir_all(next_bin.parent().unwrap()).unwrap();
        std::fs::write(&next_bin, b"#!/bin/sh\n").unwrap();
        let host = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            true,
            Some(runtime_root),
        );
        let runtime = Arc::new(RecordingMobileLinuxRuntime::default());
        let executor = MobileAppGenerationExecutor::new(Some(runtime.clone()), host);
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();

        executor.run_next_build(&layout, false).await.unwrap();

        assert!(matches!(
            runtime.recorded().network,
            traits::NetworkPolicy::Allowed
        ));
    }

    #[test]
    fn app_name_containing_a_template_marker_does_not_corrupt_the_shell() {
        let source =
            render_app_shell_source("__SPEC__", AppTemplateKind::Dashboard, &BTreeMap::new())
                .unwrap();

        assert!(source.contains("const APP_NAME = \"__SPEC__\";"));
        assert!(source.contains("const DESIGN = {"));
    }

    #[test]
    fn bundled_policy_locks_dependency_files() {
        let policy = LOCKED_FILES
            .iter()
            .map(|(path, bytes)| (PathBuf::from(path), format!("{:x}", Sha256::digest(bytes))))
            .collect::<BTreeMap<_, _>>();
        assert!(policy.contains_key(Path::new("package.json")));
        assert!(policy.contains_key(Path::new("package-lock.json")));
    }

    #[test]
    fn generated_dashboard_source_uses_runtime_and_data_bridge() {
        let source = render_app_shell_source(
            "Executive Metrics",
            AppTemplateKind::Dashboard,
            &BTreeMap::from([(
                "collection_fields".into(),
                DesignValue::DataFieldList(default_collection_fields(AppTemplateKind::Dashboard)),
            )]),
        )
        .unwrap();

        assert!(source.contains("requestRuntimeStatus"));
        assert!(source.contains("queryCollection({"));
        assert!(source.contains("sortKey: { kind: \"updated_at\" }"));
        assert!(source.contains("record.recordId"));
        assert!(source.contains("id=\"dashboard-refresh\""));
        assert!(!source.contains("应用设计摘要"));
    }

    #[test]
    fn generated_crud_source_uses_query_upsert_and_delete_controls() {
        let source = render_app_shell_source(
            "Issue Tracker",
            AppTemplateKind::CrudTracker,
            &BTreeMap::from([(
                "collection_fields".into(),
                DesignValue::DataFieldList(default_collection_fields(AppTemplateKind::CrudTracker)),
            )]),
        )
        .unwrap();

        assert!(source.contains("id=\"crud-search-input\""));
        assert!(source.contains("id=\"crud-save-record\""));
        assert!(source.contains("kind: \"upsert\""));
        assert!(source.contains("kind: \"delete\""));
        assert!(source.contains("recordId: recordId"));
        assert!(source.contains("expectedRevision"));
    }

    #[test]
    fn generated_content_source_uses_search_and_presentation_filters() {
        let source = render_app_shell_source(
            "Knowledge Base",
            AppTemplateKind::ContentShowcase,
            &BTreeMap::from([(
                "collection_fields".into(),
                DesignValue::DataFieldList(default_collection_fields(
                    AppTemplateKind::ContentShowcase,
                )),
            )]),
        )
        .unwrap();

        assert!(source.contains("id=\"content-search-input\""));
        assert!(source.contains("CONTENT_PRESENTATION === \"grid\""));
        assert!(source.contains("filterContentRecords"));
        assert!(source.contains("record.recordId"));
    }

    #[test]
    fn generated_form_source_saves_records_and_history_controls() {
        let source = render_app_shell_source(
            "Lead Intake",
            AppTemplateKind::FormUtility,
            &BTreeMap::from([(
                "collection_fields".into(),
                DesignValue::DataFieldList(default_collection_fields(AppTemplateKind::FormUtility)),
            )]),
        )
        .unwrap();

        assert!(source.contains("id=\"form-submit\""));
        assert!(source.contains("latest-submission"));
        assert!(source.contains("FORM_HISTORY_ENABLED"));
        assert!(source.contains("mutateCollection({"));
        assert!(source.contains("id=\"form-history\""));
        assert!(source.contains("recordId: recordId"));
    }

    #[tokio::test]
    async fn destructive_manifest_migration_uses_one_shot_approval_without_persisting_grant() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::AllowAlways,
            None,
        ));

        migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
            .await
            .unwrap();
        approver.await.unwrap();

        let events = sink.events().await;
        assert_eq!(events.len(), 1);
        let request = capability_request(&events[0]).unwrap();
        assert_eq!(request.capability, AppCapabilityKindDto::DataMutation);
        assert!(request.reason.contains("exact migration attempt only"));
        assert!(request.reason.contains("score"));

        let preview = AppDataStore::open(layout.clone())
            .unwrap()
            .preview_migration(&manifest_without_score())
            .unwrap();
        assert!(!preview.destructive);
        assert_eq!(
            load_permissions(&layout)
                .unwrap()
                .always_allowed_capabilities
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn destructive_manifest_migration_denial_leaves_database_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::Deny,
            None,
        ));

        let error =
            migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
                .await
                .unwrap_err();
        approver.await.unwrap();

        assert!(
            matches!(error, AppError::InvalidRequest(message) if message == "user denied destructive manifest migration")
        );
        let preview = AppDataStore::open(layout.clone())
            .unwrap()
            .preview_migration(&manifest_without_score())
            .unwrap();
        assert!(preview.destructive);
        assert_eq!(
            load_permissions(&layout)
                .unwrap()
                .always_allowed_capabilities
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn destructive_manifest_approval_is_bound_to_the_previewed_attempt() {
        let root = tempfile::tempdir().unwrap();
        let layout = AppLayout::new(root.path(), "abcd1234").unwrap();
        let mut store = AppDataStore::open(layout.clone()).unwrap();
        let old_manifest = manifest_with_score();
        store.migrate_manifest(&old_manifest, false, 1).unwrap();
        drop(store);

        let sink = MockSink::arc();
        let host =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        let approver = tokio::spawn(spawn_capability_resolution(
            sink.clone(),
            host.clone(),
            AppAuthorizationDecisionDto::AllowOnce,
            Some((layout.clone(), manifest_with_added_note())),
        ));

        let error =
            migrate_manifest_with_approval(host.as_ref(), &layout, &manifest_without_score())
                .await
                .unwrap_err();
        approver.await.unwrap();

        assert!(
            matches!(error, AppError::WorkflowStateInvalid(message) if message.contains("changed while waiting for approval"))
        );
    }

    async fn spawn_capability_resolution(
        sink: Arc<MockSink>,
        host: Arc<LocalAppsHostBroker>,
        decision: AppAuthorizationDecisionDto,
        pre_resolution_migration: Option<(AppLayout, AppManifest)>,
    ) {
        let request = wait_for_capability_request(&sink).await;
        if let Some((layout, manifest)) = pre_resolution_migration {
            let mut store = AppDataStore::open(layout).unwrap();
            store.migrate_manifest(&manifest, false, 2).unwrap();
        }
        assert!(host.resolve_capability(&request.request_id, decision).await);
    }

    async fn wait_for_capability_request(
        sink: &MockSink,
    ) -> client_protocol::local_apps::AppCapabilityRequestDto {
        loop {
            if let Some(request) = sink
                .events()
                .await
                .into_iter()
                .find_map(|event| capability_request(&event).cloned())
            {
                return request;
            }
            sleep(Duration::from_millis(10)).await;
        }
    }

    fn capability_request(
        event: &ClientEvent,
    ) -> Option<&client_protocol::local_apps::AppCapabilityRequestDto> {
        match event {
            ClientEvent::AppEvent {
                event: AppEventDto::AppCapabilityRequested { request },
            } => Some(request),
            _ => None,
        }
    }

    fn manifest_with_score() -> AppManifest {
        AppManifest {
            schema_version: local_apps::APPS_SCHEMA_VERSION,
            app_id: "abcd1234".into(),
            revision: 1,
            name: "Habits".into(),
            template: AppTemplateKind::CrudTracker,
            collections: vec![local_apps::DataCollectionSchema {
                id: "items".into(),
                name: "Items".into(),
                fields: vec![
                    data_field("title", local_apps::DataFieldKind::Text),
                    data_field("score", local_apps::DataFieldKind::Integer),
                ],
            }],
            allowed_domains: vec![],
        }
    }

    fn manifest_without_score() -> AppManifest {
        let mut manifest = manifest_with_score();
        manifest.revision = 2;
        manifest.collections[0]
            .fields
            .retain(|field| field.id != "score");
        manifest
    }

    fn manifest_with_added_note() -> AppManifest {
        let mut manifest = manifest_with_score();
        manifest.revision = 2;
        manifest.collections[0]
            .fields
            .push(data_field("note", local_apps::DataFieldKind::LongText));
        manifest
    }

    fn data_field(id: &str, kind: local_apps::DataFieldKind) -> local_apps::DataFieldSchema {
        local_apps::DataFieldSchema {
            id: id.into(),
            label: id.to_string(),
            kind,
            required: false,
            enum_options: vec![],
        }
    }
}
