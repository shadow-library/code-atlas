import { useUser } from '../hooks/useUser';

export function UserCard({ userId }: { userId: string }) {
  const { user, error } = useUser(userId);
  if (error) return <p role="alert">{error.message}</p>;
  if (!user) return <p>Loading...</p>;
  return (
    <section>
      <h2>{user.displayName}</h2>
      <p>{user.email}</p>
    </section>
  );
}
