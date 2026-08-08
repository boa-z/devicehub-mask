import type { BackendClient } from "../../shared/backend/client";
import { defaultHardwareBindings, type AppBindingConflict, type AppProfileBinding, type Profile } from "../../types";

export type ProfileList = {
  profiles: string[];
  active: string;
  app_bindings: AppProfileBinding[];
  binding_conflicts: AppBindingConflict[];
};

export class ProfileApiError extends Error {
  constructor(
    public readonly operation: string,
    public readonly status: number,
    statusText: string,
  ) {
    super(`${operation}: ${status} ${statusText}`);
  }
}

async function requireResponse(response: Response, operation: string) {
  if (!response.ok) throw new ProfileApiError(operation, response.status, response.statusText);
  return response;
}

function normalizeProfile(name: string, loaded: Profile): Profile {
  return {
    ...loaded,
    name,
    hardwareBindings: { ...defaultHardwareBindings, ...loaded.hardwareBindings },
    bundleIdentifiers: Array.isArray(loaded.bundleIdentifiers) ? loaded.bundleIdentifiers : [],
    targetResolution: loaded.targetResolution,
  };
}

export function createProfileApi(client: BackendClient) {
  return {
    async list() {
      const response = await requireResponse(await client.request("/api/profiles"), "read profiles");
      return response.json() as Promise<ProfileList>;
    },

    async read(name: string) {
      const response = await requireResponse(
        await client.request(`/api/profiles/${encodeURIComponent(name)}`),
        "read profile",
      );
      return normalizeProfile(name, await response.json() as Profile);
    },

    async write(name: string, value: Profile) {
      await requireResponse(await client.request(`/api/profiles/${encodeURIComponent(name)}`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...value, name }),
      }), "save profile");
    },

    async activate(name: string) {
      await requireResponse(await client.request(`/api/profiles/${encodeURIComponent(name)}/activate`, { method: "PUT" }), "activate profile");
    },

    async remove(name: string) {
      await requireResponse(await client.request(`/api/profiles/${encodeURIComponent(name)}/delete`, { method: "PUT" }), "delete profile");
    },
  };
}
