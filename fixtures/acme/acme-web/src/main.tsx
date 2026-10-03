import { createRoot } from 'react-dom/client';

import { OrderForm } from './components/OrderForm';
import { UserCard } from './components/UserCard';
import { useMe } from './hooks/useMe';

function App() {
  const me = useMe();
  return (
    <main>
      {me && <UserCard userId={me.id} />}
      <OrderForm />
    </main>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
