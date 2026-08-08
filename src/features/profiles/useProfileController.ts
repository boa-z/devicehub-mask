import { message } from "antd";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { TFunction } from "i18next";
import { showErrorMessage } from "../../errorMessage";
import { resolveAppProfileBinding, bindingForScope, conflictForScope, sameProfileResolution } from "../../profileBindings";
import { useUndoHistory } from "../../useUndoHistory";
import { defaultHardwareBindings, defaultProfile, type AppBindingConflict, type AppProfileBinding, type Profile, type ProfileResolution } from "../../types";
import type { BackendClient } from "../../shared/backend/client";
import { createProfileApi, ProfileApiError, type ProfileList } from "./profileApi";

function profileErrorStatus(error: unknown) {
  return error instanceof ProfileApiError
    ? error.status
    : error instanceof Error
      ? error.message
      : String(error);
}

function createLocalizedDefaultProfile(t: TFunction): Profile {
  const labels = ["mapping.defaults.move", "mapping.defaults.skill1", "mapping.defaults.skill2", "mapping.defaults.skill3"];
  return {
    ...defaultProfile,
    hardwareBindings: { ...defaultHardwareBindings },
    mappings: defaultProfile.mappings.map((mapping, index) => ({ ...mapping, label: t(labels[index]) })) as Profile["mappings"],
  };
}

type Options = {
  client: BackendClient | null;
  frameSize: ProfileResolution;
  onReleaseControls: () => void;
  t: TFunction;
};

export function useProfileController({ client, frameSize, onReleaseControls, t }: Options) {
  const translateRef = useRef(t);
  translateRef.current = t;
  const api = useMemo(() => client ? createProfileApi(client) : null, [client]);
  const initialProfileRef = useRef<Profile>(createLocalizedDefaultProfile(t));
  const {
    value: profile,
    update: updateProfile,
    reset: resetProfile,
    undo: undoProfile,
    redo: redoProfile,
    canUndo: canUndoProfile,
    canRedo: canRedoProfile,
  } = useUndoHistory<Profile>(() => initialProfileRef.current);
  const [controlProfile, setControlProfile] = useState<Profile>(profile);
  const [profiles, setProfiles] = useState<string[]>([]);
  const [activeProfile, setActiveProfile] = useState("default");
  const [profileSwitching, setProfileSwitching] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>("move");
  const [appProfileBindings, setAppProfileBindings] = useState<AppProfileBinding[]>([]);
  const [appBindingConflicts, setAppBindingConflicts] = useState<AppBindingConflict[]>([]);
  const profileSwitchingRef = useRef(false);
  const releaseControlsRef = useRef(onReleaseControls);
  releaseControlsRef.current = onReleaseControls;

  const readProfile = useCallback(async (name: string) => {
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    try {
      return await api.read(name);
    } catch (error) {
      throw new Error(translateRef.current("errors.readProfile", { status: profileErrorStatus(error) }));
    }
  }, [api]);

  const loadProfile = useCallback(async (name: string) => {
    const loaded = await readProfile(name);
    resetProfile(loaded);
    setSelectedId(loaded.mappings[0]?.id ?? null);
  }, [readProfile, resetProfile]);

  const refreshProfiles = useCallback(async () => {
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    let list: ProfileList;
    try {
      list = await api.list();
    } catch (error) {
      throw new Error(translateRef.current("errors.readProfiles", { status: profileErrorStatus(error) }));
    }
    setProfiles(list.profiles);
    setActiveProfile(list.active);
    setAppProfileBindings(list.app_bindings ?? []);
    setAppBindingConflicts(list.binding_conflicts ?? []);
    return list;
  }, [api]);

  const activateSavedControlProfile = useCallback(async (target: string) => {
    if (target === activeProfile) return false;
    if (profileSwitchingRef.current) throw new Error(translateRef.current("profile.switchInProgress"));
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    profileSwitchingRef.current = true;
    setProfileSwitching(target);
    try {
      const loaded = await readProfile(target);
      releaseControlsRef.current();
      try {
        await api.activate(target);
      } catch (error) {
        throw new Error(translateRef.current("errors.activateProfile", { status: profileErrorStatus(error) }));
      }
      setActiveProfile(target);
      setControlProfile(loaded);
      return true;
    } finally {
      profileSwitchingRef.current = false;
      setProfileSwitching(null);
    }
  }, [activeProfile, api, readProfile]);

  const activateProfileForApp = useCallback(async (bundleId: string, currentFrameSize = frameSize) => {
    const { binding, conflict } = resolveAppProfileBinding(bundleId, currentFrameSize, appProfileBindings, appBindingConflicts);
    if (!binding || conflict) return;
    try {
      if (await activateSavedControlProfile(binding.profile)) {
        void message.success(translateRef.current("profile.autoActivated", { profile: binding.profile }));
      }
    } catch (error) {
      void message.warning(translateRef.current("profile.autoActivateFailed", { error: String(error) }));
    }
  }, [activateSavedControlProfile, appBindingConflicts, appProfileBindings, frameSize]);

  const switchControlProfile = useCallback(async (target: string) => {
    try {
      if (await activateSavedControlProfile(target)) {
        void message.success(translateRef.current("profile.switched", { profile: target }));
      }
    } catch (error) {
      void showErrorMessage(translateRef.current("profile.switchFailed", { error: String(error) }));
    }
  }, [activateSavedControlProfile]);

  const writeProfile = useCallback(async (name: string, value: Profile) => {
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    try {
      await api.write(name, value);
    } catch (error) {
      throw new Error(translateRef.current("errors.saveProfile", { status: profileErrorStatus(error) }));
    }
  }, [api]);

  const changeAppProfileBinding = useCallback(async (bundleId: string, bind: boolean) => {
    const scope = frameSize;
    if (conflictForScope(bundleId, scope, appBindingConflicts)) throw new Error(translateRef.current("profile.appBindingConflict"));
    const owner = bindingForScope(bundleId, scope, appProfileBindings)?.profile;
    const profileName = bind ? activeProfile : owner;
    if (!profileName || (bind && owner && owner !== activeProfile)) {
      throw new Error(translateRef.current("profile.appBindingOwned", { profile: owner ?? "" }));
    }
    const loaded = await readProfile(profileName);
    if (bind && loaded.targetResolution !== null && !sameProfileResolution(loaded.targetResolution, frameSize)) {
      throw new Error(translateRef.current("profile.resolutionMismatch", {
        width: loaded.targetResolution.width,
        height: loaded.targetResolution.height,
      }));
    }
    const bundleIdentifiers = bind
      ? [...new Set([...loaded.bundleIdentifiers, bundleId])]
      : loaded.bundleIdentifiers.filter((candidate) => candidate !== bundleId);
    const updated = {
      ...loaded,
      bundleIdentifiers,
      targetResolution: bundleIdentifiers.length > 0 ? (loaded.targetResolution ?? { ...frameSize }) : null,
    };
    await writeProfile(profileName, updated);
    await refreshProfiles();
    const mergeBinding = (current: Profile) => current.name === profileName
      ? {
          ...current,
          bundleIdentifiers: bind
            ? [...new Set([...current.bundleIdentifiers, bundleId])]
            : current.bundleIdentifiers.filter((candidate) => candidate !== bundleId),
          targetResolution: updated.targetResolution,
        }
      : current;
    resetProfile(mergeBinding(profile));
    setControlProfile(mergeBinding);
  }, [activeProfile, appBindingConflicts, appProfileBindings, frameSize, profile, readProfile, refreshProfiles, resetProfile, writeProfile]);

  useEffect(() => {
    if (!api) return;
    const initializeProfiles = async () => {
      const list = await refreshProfiles();
      if (list.profiles.length === 0) {
        const initialProfile = initialProfileRef.current;
        await writeProfile("default", initialProfile);
        await api.activate("default");
        setProfiles(["default"]);
        setActiveProfile("default");
        resetProfile(initialProfile);
        setControlProfile(initialProfile);
        return;
      }
      const selected = list.profiles.includes(list.active) ? list.active : list.profiles[0];
      const loaded = await readProfile(selected);
      resetProfile(loaded);
      setControlProfile(loaded);
      setSelectedId(loaded.mappings[0]?.id ?? null);
    };
    void initializeProfiles().catch((error) => showErrorMessage(error));
  }, [api, readProfile, refreshProfiles, resetProfile, writeProfile]);

  const save = useCallback(async () => {
    try {
      await writeProfile(profile.name, profile);
      await refreshProfiles();
      resetProfile(profile);
      if (activeProfile === profile.name) {
        releaseControlsRef.current();
        setControlProfile(profile);
      }
      void message.success(translateRef.current("mapping.saved"));
    } catch (error) {
      void showErrorMessage(error);
    }
  }, [activeProfile, profile, refreshProfiles, resetProfile, writeProfile]);

  const activateCurrentProfile = useCallback(async () => {
    releaseControlsRef.current();
    await writeProfile(profile.name, profile);
    await refreshProfiles();
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    try {
      await api.activate(profile.name);
    } catch (error) {
      throw new Error(translateRef.current("errors.activateProfile", { status: profileErrorStatus(error) }));
    }
    setActiveProfile(profile.name);
    setControlProfile(profile);
    resetProfile(profile);
    void message.success(translateRef.current("mapping.activated"));
  }, [api, profile, refreshProfiles, resetProfile, writeProfile]);

  const createProfile = useCallback(async (name: string) => {
    const next: Profile = { ...defaultProfile, name, mappings: [], hardwareBindings: { ...defaultHardwareBindings } };
    await writeProfile(name, next);
    await refreshProfiles();
    await loadProfile(name);
  }, [loadProfile, refreshProfiles, writeProfile]);

  const duplicateProfile = useCallback(async (name: string) => {
    await writeProfile(name, { ...profile, name, bundleIdentifiers: [], targetResolution: null });
    await refreshProfiles();
    await loadProfile(name);
  }, [loadProfile, profile, refreshProfiles, writeProfile]);

  const renameProfile = useCallback(async (name: string) => {
    const oldName = profile.name;
    if (name === oldName) return;
    await writeProfile(name, { ...profile, name });
    if (activeProfile === oldName) {
      releaseControlsRef.current();
      if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
      try {
        await api.activate(name);
      } catch (error) {
        throw new Error(translateRef.current("errors.activateProfile", { status: profileErrorStatus(error) }));
      }
      setActiveProfile(name);
      setControlProfile({ ...profile, name });
    }
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    try {
      await api.remove(oldName);
    } catch (error) {
      throw new Error(translateRef.current("errors.deleteOldProfile", { status: profileErrorStatus(error) }));
    }
    await refreshProfiles();
    await loadProfile(name);
  }, [activeProfile, api, loadProfile, profile, refreshProfiles, writeProfile]);

  const deleteCurrentProfile = useCallback(async () => {
    if (!api) throw new Error(translateRef.current("errors.backendNotReady"));
    try {
      await api.remove(profile.name);
    } catch (error) {
      throw new Error(translateRef.current("errors.deleteProfile", { status: profileErrorStatus(error) }));
    }
    setProfiles((current) => current.filter((name) => name !== profile.name));
    resetProfile(controlProfile);
    setSelectedId(controlProfile.mappings[0]?.id ?? null);
  }, [api, controlProfile, profile.name, resetProfile]);

  const importProfile = useCallback(async (next: Profile, imported: number, skipped: number) => {
    await writeProfile(next.name, next);
    await refreshProfiles();
    resetProfile(next);
    setSelectedId(next.mappings[0]?.id ?? null);
    if (activeProfile === next.name) {
      releaseControlsRef.current();
      setControlProfile(next);
    }
    void message.success(translateRef.current(skipped ? "mapping.importedWithSkipped" : "mapping.imported", { imported, skipped }));
  }, [activeProfile, refreshProfiles, resetProfile, writeProfile]);

  const installCatalogProfile = useCallback(async (name: string) => {
    await refreshProfiles();
    await loadProfile(name);
  }, [loadProfile, refreshProfiles]);

  return {
    profile,
    controlProfile,
    profiles,
    activeProfile,
    profileSwitching,
    selectedId,
    setSelectedId,
    appProfileBindings,
    appBindingConflicts,
    updateProfile,
    resetProfile,
    undoProfile,
    redoProfile,
    canUndoProfile,
    canRedoProfile,
    loadProfile,
    save,
    activateCurrentProfile,
    createProfile,
    duplicateProfile,
    renameProfile,
    deleteCurrentProfile,
    importProfile,
    installCatalogProfile,
    switchControlProfile,
    activateProfileForApp,
    changeAppProfileBinding,
  };
}
