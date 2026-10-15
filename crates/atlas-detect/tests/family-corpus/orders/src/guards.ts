export class SessionGuard {
  canPass(): boolean {
    return true;
  }
}

export class AdminGuard {
  canPass(): boolean {
    return false;
  }
}
