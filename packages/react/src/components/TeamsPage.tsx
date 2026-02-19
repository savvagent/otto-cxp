import { useState } from "react";
import { useTeams } from "../hooks/useTeams.js";

export interface TeamsPageLabels {
  title?: string;
  createButton?: string;
  editButton?: string;
  deleteButton?: string;
  confirmDelete?: string;
  cancelButton?: string;
  saveButton?: string;
  nameLabel?: string;
  descriptionLabel?: string;
  ownerLabel?: string;
  searchPlaceholder?: string;
  noTeams?: string;
  loadingText?: string;
  createdAt?: string;
}

export interface TeamsPageProps {
  labels?: TeamsPageLabels;
}

const defaultLabels: Required<TeamsPageLabels> = {
  title: "Teams",
  createButton: "Create Team",
  editButton: "Edit",
  deleteButton: "Delete",
  confirmDelete: "Are you sure you want to delete this team?",
  cancelButton: "Cancel",
  saveButton: "Save",
  nameLabel: "Name",
  descriptionLabel: "Description",
  ownerLabel: "Owner",
  searchPlaceholder: "Search teams...",
  noTeams: "No teams found",
  loadingText: "Loading...",
  createdAt: "Created",
};

interface FormState {
  name: string;
  description: string;
  owner: string;
}

const emptyForm: FormState = { name: "", description: "", owner: "" };

export function TeamsPage({ labels: labelsProp }: TeamsPageProps) {
  const l = { ...defaultLabels, ...labelsProp };
  const { teams, loading, error, createTeam, updateTeam, deleteTeam } =
    useTeams();

  const [search, setSearch] = useState("");
  const [showForm, setShowForm] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);

  const filtered = teams.filter(
    (t) =>
      t.name.toLowerCase().includes(search.toLowerCase()) ||
      (t.description ?? "").toLowerCase().includes(search.toLowerCase()) ||
      (t.owner ?? "").toLowerCase().includes(search.toLowerCase())
  );

  const handleSave = async () => {
    if (!form.name.trim()) return;
    try {
      if (editingId) {
        await updateTeam(editingId, {
          name: form.name,
          description: form.description || undefined,
          owner: form.owner || undefined,
        });
      } else {
        await createTeam(form.name, {
          description: form.description || undefined,
          owner: form.owner || undefined,
        });
      }
      setShowForm(false);
      setEditingId(null);
      setForm(emptyForm);
    } catch {
      // error is set via hook
    }
  };

  const handleEdit = (team: { id: string; name: string; description?: string; owner?: string }) => {
    setEditingId(team.id);
    setForm({
      name: team.name,
      description: team.description ?? "",
      owner: team.owner ?? "",
    });
    setShowForm(true);
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteTeam(id);
    } catch {
      // error is set via hook
    }
    setDeleteConfirmId(null);
  };

  const handleCancel = () => {
    setShowForm(false);
    setEditingId(null);
    setForm(emptyForm);
  };

  return (
    <div style={{ fontFamily: "system-ui, sans-serif", maxWidth: 900, margin: "0 auto" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 16 }}>
        <h2 style={{ margin: 0 }}>{l.title}</h2>
        <button
          onClick={() => { setShowForm(true); setEditingId(null); setForm(emptyForm); }}
          style={{
            background: "#1976d2",
            color: "#fff",
            border: "none",
            borderRadius: 4,
            padding: "8px 16px",
            cursor: "pointer",
            fontSize: 14,
          }}
        >
          {l.createButton}
        </button>
      </div>

      <input
        type="text"
        placeholder={l.searchPlaceholder}
        value={search}
        onChange={(e) => setSearch(e.target.value)}
        style={{
          width: "100%",
          padding: "8px 12px",
          border: "1px solid #e0e0e0",
          borderRadius: 4,
          marginBottom: 16,
          fontSize: 14,
          boxSizing: "border-box",
        }}
      />

      {error && (
        <div style={{ color: "#d32f2f", marginBottom: 12, fontSize: 14 }}>
          {error.message}
        </div>
      )}

      {showForm && (
        <div
          style={{
            border: "1px solid #e0e0e0",
            borderRadius: 4,
            padding: 16,
            marginBottom: 16,
            background: "#fafafa",
          }}
        >
          <div style={{ marginBottom: 12 }}>
            <label style={{ display: "block", fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
              {l.nameLabel}
            </label>
            <input
              type="text"
              value={form.name}
              onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
              style={{
                width: "100%",
                padding: "6px 10px",
                border: "1px solid #e0e0e0",
                borderRadius: 4,
                fontSize: 14,
                boxSizing: "border-box",
              }}
            />
          </div>
          <div style={{ marginBottom: 12 }}>
            <label style={{ display: "block", fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
              {l.descriptionLabel}
            </label>
            <textarea
              value={form.description}
              onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
              rows={2}
              style={{
                width: "100%",
                padding: "6px 10px",
                border: "1px solid #e0e0e0",
                borderRadius: 4,
                fontSize: 14,
                resize: "vertical",
                boxSizing: "border-box",
              }}
            />
          </div>
          <div style={{ marginBottom: 12 }}>
            <label style={{ display: "block", fontSize: 13, fontWeight: 600, marginBottom: 4 }}>
              {l.ownerLabel}
            </label>
            <input
              type="text"
              value={form.owner}
              onChange={(e) => setForm((f) => ({ ...f, owner: e.target.value }))}
              style={{
                width: "100%",
                padding: "6px 10px",
                border: "1px solid #e0e0e0",
                borderRadius: 4,
                fontSize: 14,
                boxSizing: "border-box",
              }}
            />
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <button
              onClick={handleSave}
              style={{
                background: "#1976d2",
                color: "#fff",
                border: "none",
                borderRadius: 4,
                padding: "6px 16px",
                cursor: "pointer",
                fontSize: 14,
              }}
            >
              {l.saveButton}
            </button>
            <button
              onClick={handleCancel}
              style={{
                background: "#fff",
                color: "#333",
                border: "1px solid #e0e0e0",
                borderRadius: 4,
                padding: "6px 16px",
                cursor: "pointer",
                fontSize: 14,
              }}
            >
              {l.cancelButton}
            </button>
          </div>
        </div>
      )}

      {loading ? (
        <div style={{ textAlign: "center", padding: 24, color: "#999" }}>
          {l.loadingText}
        </div>
      ) : filtered.length === 0 ? (
        <div style={{ textAlign: "center", padding: 24, color: "#999" }}>
          {l.noTeams}
        </div>
      ) : (
        <table
          style={{
            width: "100%",
            borderCollapse: "collapse",
            border: "1px solid #e0e0e0",
            fontSize: 14,
          }}
        >
          <thead>
            <tr style={{ background: "#f5f5f5" }}>
              <th style={{ textAlign: "left", padding: "8px 12px", borderBottom: "1px solid #e0e0e0" }}>
                {l.nameLabel}
              </th>
              <th style={{ textAlign: "left", padding: "8px 12px", borderBottom: "1px solid #e0e0e0" }}>
                {l.descriptionLabel}
              </th>
              <th style={{ textAlign: "left", padding: "8px 12px", borderBottom: "1px solid #e0e0e0" }}>
                {l.ownerLabel}
              </th>
              <th style={{ textAlign: "left", padding: "8px 12px", borderBottom: "1px solid #e0e0e0" }}>
                {l.createdAt}
              </th>
              <th style={{ textAlign: "right", padding: "8px 12px", borderBottom: "1px solid #e0e0e0" }} />
            </tr>
          </thead>
          <tbody>
            {filtered.map((team) => (
              <tr key={team.id} style={{ borderBottom: "1px solid #e0e0e0" }}>
                <td style={{ padding: "8px 12px", fontWeight: 500 }}>{team.name}</td>
                <td style={{ padding: "8px 12px", color: "#666" }}>
                  {team.description ?? "—"}
                </td>
                <td style={{ padding: "8px 12px", color: "#666" }}>
                  {team.owner ?? "—"}
                </td>
                <td style={{ padding: "8px 12px", color: "#999", fontSize: 13 }}>
                  {new Date(team.createdAt).toLocaleDateString()}
                </td>
                <td style={{ padding: "8px 12px", textAlign: "right", whiteSpace: "nowrap" }}>
                  {deleteConfirmId === team.id ? (
                    <>
                      <span style={{ fontSize: 13, marginRight: 8 }}>{l.confirmDelete}</span>
                      <button
                        onClick={() => handleDelete(team.id)}
                        style={{
                          background: "#d32f2f",
                          color: "#fff",
                          border: "none",
                          borderRadius: 4,
                          padding: "4px 10px",
                          cursor: "pointer",
                          fontSize: 13,
                          marginRight: 4,
                        }}
                      >
                        {l.deleteButton}
                      </button>
                      <button
                        onClick={() => setDeleteConfirmId(null)}
                        style={{
                          background: "#fff",
                          color: "#333",
                          border: "1px solid #e0e0e0",
                          borderRadius: 4,
                          padding: "4px 10px",
                          cursor: "pointer",
                          fontSize: 13,
                        }}
                      >
                        {l.cancelButton}
                      </button>
                    </>
                  ) : (
                    <>
                      <button
                        onClick={() => handleEdit(team)}
                        style={{
                          background: "transparent",
                          color: "#1976d2",
                          border: "none",
                          cursor: "pointer",
                          fontSize: 13,
                          marginRight: 8,
                        }}
                      >
                        {l.editButton}
                      </button>
                      <button
                        onClick={() => setDeleteConfirmId(team.id)}
                        style={{
                          background: "transparent",
                          color: "#d32f2f",
                          border: "none",
                          cursor: "pointer",
                          fontSize: 13,
                        }}
                      >
                        {l.deleteButton}
                      </button>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
