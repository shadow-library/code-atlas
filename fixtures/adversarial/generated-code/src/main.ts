import { createPetClient, type Pet } from './generated/api-types';

export async function loadPet(id: number): Promise<Pet> {
  return createPetClient('https://pets.example.test').getPet(id);
}
