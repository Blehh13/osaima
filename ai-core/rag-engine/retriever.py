import sys
import json
import os
import chromadb
from sentence_transformers import SentenceTransformer

def main():
    if len(sys.argv) < 2:
        print(json.dumps({"error": "No query provided"}))
        sys.exit(1)

    query = sys.argv[1]
    
    BASE_DIR = os.environ.get("OSAIMA_ROOT", os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..")))
    DB_DIR = os.path.join(BASE_DIR, "ai-core", "rag-engine", "chroma_db")

    if not os.path.exists(DB_DIR):
        print(json.dumps({"context": []}))
        sys.exit(0)

    try:
        client = chromadb.PersistentClient(path=DB_DIR)
        collection = client.get_collection(name="osaima_knowledge")
        model = SentenceTransformer('all-MiniLM-L6-v2')
        
        query_embedding = model.encode([query]).tolist()
        
        results = collection.query(
            query_embeddings=query_embedding,
            n_results=3
        )
        
        context_chunks = []
        if results and 'documents' in results and results['documents']:
            for i, doc in enumerate(results['documents'][0]):
                meta = results['metadatas'][0][i] if results['metadatas'] else {}
                context_chunks.append({
                    "text": doc,
                    "metadata": meta
                })
                
        print(json.dumps({"context": context_chunks}))
    except Exception as e:
        print(json.dumps({"error": str(e)}))
        sys.exit(1)

if __name__ == "__main__":
    main()
