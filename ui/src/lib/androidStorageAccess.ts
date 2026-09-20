interface AndroidFolderPickerBridge {
  hasPersistentStorageAccess?: () => boolean;
  requestPersistentStorageAccess?: () => void;
}

function bridge(): AndroidFolderPickerBridge | undefined {
  return (window as typeof window & { FolderPickerBridge?: AndroidFolderPickerBridge }).FolderPickerBridge;
}

export function canRequestPersistentStorageAccess(): boolean {
  return typeof bridge()?.requestPersistentStorageAccess === "function";
}

export function hasPersistentStorageAccess(): boolean {
  return bridge()?.hasPersistentStorageAccess?.() ?? true;
}

export function requestPersistentStorageAccess(): boolean {
  const request = bridge()?.requestPersistentStorageAccess;
  if (!request) return false;
  request();
  return true;
}

export function isStoragePermissionError(error: string | null): boolean {
  return !!error && /operation not permitted|permission denied/i.test(error);
}
